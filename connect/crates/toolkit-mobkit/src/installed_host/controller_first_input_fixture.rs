//! Source-only test composition from the real installed host. Provisioning uses
//! native durable grants; it does not test an operator presenter or OS login.
use super::*;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_core::ControllerModelFacts;
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy;
use toolkit_package::Id;

pub(crate) struct Host {
    pub selection: HostBootstrapSelection,
    pub store: Arc<SqliteStore>,
    pub source_path: PathBuf,
    pub source_scope: StoreScope,
    pub tool_policy_registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
    pub tool_policy: PreparedInstalledToolPolicy,
    pub directory: Arc<tempfile::TempDir>,
    pub lifetime: Arc<NativeHostLifetime>,
    pub originals: Arc<PrivateOriginals>,
    pub configuration: NativeGrantWorkConfiguration,
    policy: Arc<InstalledApplicationPolicy>,
    pub registration: Arc<dyn MemberRuntimeRegistrationHook>,
    pub acquisition_helper: Option<acquisition::InstalledSetupSelection>,
}

impl Host {
    /// Test-only fixture selection before this configuration is handed to the
    /// persistent service. All installed source, native and lifetime owners
    /// stay identical; this optional floor can only raise review.
    pub(crate) fn select_fixture_review_before_service(
        &mut self,
        review: Arc<dyn InstalledOperationReviewPolicy>,
    ) {
        assert!(self.policy.operation_review_policy.is_none());
        let policy = Arc::new(InstalledApplicationPolicy {
            operation_review_policy: Some(review),
            originals: self.policy.originals.clone(),
            lifetime: self.policy.lifetime.clone(),
            native: self.policy.native.clone(),
            selected: self.policy.selected.clone(),
        });
        assert!(Arc::ptr_eq(&policy.originals, &self.originals));
        assert!(Arc::ptr_eq(&policy.lifetime, &self.lifetime));
        self.configuration = self.originals.configuration(policy.clone());
        self.policy = policy;
    }
}

pub(crate) async fn open_with_probe(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
) -> Host {
    open_selected(facts, select_controller, probe, false).await
}

pub(crate) async fn open_for_service_input(facts: &ControllerModelFacts) -> Host {
    open_selected(facts, true, true, true).await
}

pub(crate) async fn open_for_service_input_with_source(
    facts: &ControllerModelFacts,
    prepare_source: PrepareOriginalSource,
) -> Host {
    open_selected_subject_with_source(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        false,
        false,
        Some(prepare_source),
    )
    .await
}

/// Select the ordinary human-reviewed member-policy recovery owner before the
/// provider or native configuration is shared. Service access still needs its
/// actual human decision; this selects no standing service authority.
pub(crate) async fn open_for_service_input_with_human_policy(facts: &ControllerModelFacts) -> Host {
    open_selected_source(
        facts, true, true, true, false, None, false, None, false, false, None, None, false, false,
        true, false,
    )
    .await
}

async fn open_selected(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
) -> Host {
    open_selected_subject(
        facts,
        select_controller,
        probe,
        local_operator,
        false,
        None,
        false,
        None,
        false,
        false,
    )
    .await
}

pub(crate) async fn open_for_source_status(facts: &ControllerModelFacts) -> Host {
    open_selected_subject(
        facts, true, true, true, true, None, false, None, false, false,
    )
    .await
}

pub(crate) async fn open_with_acquisition(
    facts: &ControllerModelFacts,
    tool: Arc<dyn meerkat_core::AgentToolDispatcher>,
) -> Host {
    open_selected_subject(
        facts,
        true,
        false,
        false,
        false,
        Some(tool),
        false,
        None,
        false,
        false,
    )
    .await
}

async fn open_selected_subject(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
    source_subject: bool,
    acquisition: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    helper: bool,
    report: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    configured_setup: bool,
    configured_report: bool,
) -> Host {
    open_selected_subject_with_source(
        facts,
        select_controller,
        probe,
        local_operator,
        source_subject,
        acquisition,
        helper,
        report,
        configured_setup,
        configured_report,
        None,
    )
    .await
}

// Test-only storage decoration selects the actual source store before its
// first adapter is constructed, claimed, or shared with native configuration.
pub(crate) type PrepareOriginalSource = Box<
    dyn FnOnce(Arc<dyn toolkit_store::ToolkitStore>) -> Arc<dyn toolkit_store::ToolkitStore> + Send,
>;

async fn open_selected_subject_with_source(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
    source_subject: bool,
    acquisition: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    helper: bool,
    report: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    configured_setup: bool,
    configured_report: bool,
    prepare_source: Option<PrepareOriginalSource>,
) -> Host {
    open_selected_source(
        facts,
        select_controller,
        probe,
        local_operator,
        source_subject,
        acquisition,
        helper,
        report,
        configured_setup,
        configured_report,
        prepare_source,
        None,
        false,
        false,
        false,
        false,
    )
    .await
}

/// The fixture selects an issuer-qualified Human before constructing the host.
/// Only the later real verifier can authenticate a token for that selection.
pub(crate) async fn open_for_console_input(
    facts: &ControllerModelFacts,
    human: meerkat_core::PrincipalRef,
) -> Host {
    assert_eq!(human.kind, meerkat_core::PrincipalKind::Human);
    open_selected_source(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        false,
        false,
        None,
        Some(human),
        false,
        false,
        false,
        false,
    )
    .await
}

/// Select the exact future verified human as the native operator/root before
/// initializing any owner. Ordinary Console input fixtures remain unchanged.
pub(crate) async fn open_for_console_operator_input(
    facts: &ControllerModelFacts,
    human: meerkat_core::PrincipalRef,
) -> Host {
    assert_eq!(human.kind, meerkat_core::PrincipalKind::Human);
    open_selected_source(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        true,
        false,
        None,
        Some(human),
        false,
        false,
        false,
        false,
    )
    .await
}

/// Public batch fixture uses the normal human-reviewed policy participant and
/// a genuinely verified Console Original. No service authority is preissued.
pub(crate) async fn open_for_console_service_batch(
    facts: &ControllerModelFacts,
    human: meerkat_core::PrincipalRef,
) -> Host {
    assert_eq!(human.kind, meerkat_core::PrincipalKind::Human);
    open_selected_source(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        true,
        false,
        None,
        Some(human),
        false,
        false,
        true,
        false,
    )
    .await
}

/// Explicit fixture administration for an actual fresh task. This does not
/// authenticate a Human or preselect an application parent for a future session.
pub(crate) async fn open_for_task_parent_service_input(facts: &ControllerModelFacts) -> Host {
    open_selected_source(
        facts, true, true, true, false, None, false, None, false, false, None, None, true, false,
        false, false,
    )
    .await
}

async fn open_selected_source(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
    source_subject: bool,
    acquisition: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    helper: bool,
    report: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    configured_setup: bool,
    configured_report: bool,
    prepare_source: Option<PrepareOriginalSource>,
    console_human: Option<meerkat_core::PrincipalRef>,
    task_parent_administration: bool,
    private_helper_entry: bool,
    human_policy: bool,
    catalog_probe: bool,
) -> Host {
    open_selected_source_with_profile(
        facts,
        select_controller,
        probe,
        local_operator,
        source_subject,
        acquisition,
        helper,
        report,
        configured_setup,
        configured_report,
        prepare_source,
        console_human,
        task_parent_administration,
        private_helper_entry,
        human_policy,
        catalog_probe,
        None,
    )
    .await
}

#[derive(Clone, Copy)]
pub(crate) enum WarmHelperProfile {
    Standard,
    SameExecutor,
    CrossAudience,
}

async fn open_selected_source_with_profile(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
    source_subject: bool,
    acquisition: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    helper: bool,
    report: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    configured_setup: bool,
    configured_report: bool,
    prepare_source: Option<PrepareOriginalSource>,
    console_human: Option<meerkat_core::PrincipalRef>,
    task_parent_administration: bool,
    private_helper_entry: bool,
    human_policy: bool,
    catalog_probe: bool,
    warm_profile: Option<WarmHelperProfile>,
) -> Host {
    open_selected_source_with_workflow_members(
        facts,
        select_controller,
        probe,
        local_operator,
        source_subject,
        acquisition,
        helper,
        report,
        configured_setup,
        configured_report,
        prepare_source,
        console_human,
        task_parent_administration,
        private_helper_entry,
        human_policy,
        catalog_probe,
        warm_profile,
        false,
    )
    .await
}

/// The dedicated path retains the actual public installed composition until
/// its caller has reserved the selected member. No lifetime is prebound here.
pub(crate) struct DedicatedHost {
    pub selection: HostBootstrapSelection,
    pub store: Arc<SqliteStore>,
    pub source_path: PathBuf,
    pub source_scope: StoreScope,
    pub directory: Arc<tempfile::TempDir>,
    pub lifetime: Arc<NativeHostLifetime>,
    pub originals: Arc<PrivateOriginals>,
    pub installed: Option<InstalledToolkitHost>,
}

impl DedicatedHost {
    /// Adversarial composition over the same real source and selected artifacts.
    /// Only this losing host uses the fixture's private artifact access. The
    /// accepted host below exercises public open/compose without prebinding.
    pub fn competing_gateway(
        &self,
        lifetime: Arc<NativeHostLifetime>,
        closed: PrivateHostClosed,
    ) -> ToolkitGatewayHost {
        let installed = self.installed.as_ref().unwrap();
        let policy = Arc::new(installed.final_policy(lifetime.clone()));
        let registration = Arc::new(InstalledMemberRegistration {
            originals: installed.originals.clone(),
            owner: lifetime.clone(),
            native: installed.native.clone(),
            surfaces: installed.surfaces.clone(),
            tool_policy: installed.tool_policy.clone(),
            update_associations: installed.update_associations.clone(),
        });
        ToolkitGatewayHost::with_private_originals(
            lifetime,
            closed,
            self.originals.clone(),
            policy,
            registration,
        )
        .with_installed_tool_policy(installed.tool_policy.registry())
    }
}

pub(crate) async fn open_for_dedicated_workflow(facts: &ControllerModelFacts) -> DedicatedHost {
    let (directory, selection, helper, prepare) = provision_selected_source_with_workflow_members(
        facts, true, true, true, false, None, false, None, false, false, None, None, false, false,
        false, false, None, true,
    );
    assert!(helper.is_none() && prepare.is_none());
    reopen_for_dedicated_workflow(directory, selection).await
}

pub(crate) async fn open_for_operational_dedicated_workflow(
    facts: &ControllerModelFacts,
) -> DedicatedHost {
    let (directory, mut selection, helper, prepare) =
        provision_selected_source_with_workflow_members(
            facts, true, true, true, false, None, false, None, false, false, None, None, false,
            false, false, true, None, true,
        );
    assert!(helper.is_none() && prepare.is_none());
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    local.native_host.as_mut().unwrap().member_classes = vec![
        meerkat_toolkit::host_lifetime::policy::CompiledMemberClass {
            id: Id::new("operational-parent-class").unwrap(),
            purpose:
                meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow,
            profile: "worker".into(),
            factory: Id::new("host:planning").unwrap(),
            callback_account: None,
            callbacks: vec![],
            grants: [
                "setup_state",
                "tool_catalog_load",
                "tool_catalog_search",
                "toolkit_request_public_service",
            ]
            .into_iter()
            .map(|name| meerkat_core::CompiledMemberToolGrant {
                tool_name: name.into(),
                action: meerkat_core::CompiledMemberToolAction::Invoke,
                consequence: meerkat_core::CompiledToolConsequence::R2,
            })
            .collect(),
        },
    ];
    super::tests::replace_local(&mut selection, &local);
    reopen_for_dedicated_workflow(directory, selection).await
}

pub(crate) async fn open_for_configured_dedicated_workflow(
    facts: &ControllerModelFacts,
) -> DedicatedHost {
    let (directory, selection, helper, prepare) = provision_selected_source_with_workflow_members(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        true,
        false,
        None,
        Some(super::tests::principal("workflow-human")),
        false,
        false,
        false,
        false,
        None,
        true,
    );
    assert!(helper.is_some() && prepare.is_none());
    reopen_for_dedicated_workflow(directory, selection).await
}

/// Explicit unit-test composition: the selected surface is the same actual
/// enrolled Find leaf used by the persistent native workflow fixture.
pub(crate) async fn open_for_reviewed_dedicated_workflow(
    facts: &ControllerModelFacts,
    acquisition: Arc<dyn meerkat_core::AgentToolDispatcher>,
) -> (DedicatedHost, acquisition::InstalledSetupSelection) {
    let (directory, selection, helper, prepare) = provision_selected_source_with_workflow_members(
        facts,
        true,
        false,
        true,
        false,
        Some(acquisition),
        false,
        None,
        true,
        false,
        None,
        Some(super::tests::principal("workflow-human")),
        false,
        false,
        false,
        false,
        None,
        true,
    );
    assert!(prepare.is_none());
    (
        reopen_for_dedicated_workflow(directory, selection).await,
        helper.unwrap(),
    )
}

/// Fresh explicit shared profile through the real stopped-owner producer.
/// The two real native MCP roots are component-test inputs, not live services.
pub(crate) async fn open_for_effect_dedicated_workflow(
    facts: &ControllerModelFacts,
) -> DedicatedHost {
    open_for_effect_dedicated_workflow_selected(facts, false, false).await
}

/// Explicit public-service selection only for the workflow origin regression.
pub(crate) async fn open_for_public_dedicated_workflow(
    facts: &ControllerModelFacts,
) -> DedicatedHost {
    open_for_effect_dedicated_workflow_selected(facts, true, false).await
}

/// Case20 keeps the public waiter but selects a finite class for P's genuine
/// operation ceiling before any original is admitted.
pub(crate) async fn open_for_operational_public_dedicated_workflow(
    facts: &ControllerModelFacts,
) -> DedicatedHost {
    open_for_effect_dedicated_workflow_selected(facts, true, true).await
}

async fn open_for_effect_dedicated_workflow_selected(
    facts: &ControllerModelFacts,
    public: bool,
    operational_parent: bool,
) -> DedicatedHost {
    use meerkat_authorization_contracts::work_association::{
        GrantTarget, TargetGrantLineage, WorkAuthorityBasis,
    };
    let (directory, mut selection, _, _) = provision_selected_source_with_workflow_members(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        false,
        false,
        None,
        Some(super::tests::principal("workflow-human")),
        false,
        false,
        false,
        false,
        None,
        true,
    );
    // Select the same actual operator that initialized these durable grants.
    // The base controller-only fixture deliberately has no remote setup owner.
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let native = local.native_host.as_mut().unwrap();
    let operator =
        meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
            .unwrap()
            .operator;
    assert_eq!(
        native.routes[0].controller_parent.as_ref().unwrap()[0].root_authority,
        operator
    );
    native.remote_operator = Some(operator);
    if operational_parent {
        assert!(public);
        assert!(
            !native
                .member_classes
                .iter()
                .any(|class| class.id.as_str() == "operational-parent-class")
        );
        native.member_classes.push(meerkat_toolkit::host_lifetime::policy::CompiledMemberClass {
            id: Id::new("operational-parent-class").unwrap(),
            purpose: meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow,
            profile: "worker".into(), factory: Id::new("host:planning").unwrap(),
            callback_account: None, callbacks: vec![],
            grants: ["setup_state", "tool_catalog_load", "tool_catalog_search", "toolkit_request_public_service"]
                .into_iter().map(|name| meerkat_core::CompiledMemberToolGrant {
                    tool_name: name.into(), action: meerkat_core::CompiledMemberToolAction::Invoke,
                    consequence: meerkat_core::CompiledToolConsequence::R2,
                }).collect(),
        });
    }
    super::tests::replace_local(&mut selection, &local);
    let capability = NativeHostLifetime::with_candidate_installation(&selection, |policy| {
        let native = policy.native_host.as_ref().unwrap();
        let operator = native.remote_operator.as_ref().unwrap();
        let directory = state_directory(&selection, native, NativeOwnerStartup::Restore)?;
        let (source, originals) = operator_setup::open_owner_pair_in(
            &directory,
            &native.ingress_domain,
            Some(operator),
            operator_setup::OperatorOwnerStartup::Restore,
            false,
        )?;
        let member = &native.members[2];
        let route = &native.routes[2];
        let mut targets = Vec::new();
        let mut requests = Vec::new();
        for server in ["restore-read-a", "restore-read-b"] {
            let parent = source
                .grants()
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    operator,
                    EvidenceId::new(format!("helper-{server}")).unwrap(),
                    route.logical_executor.clone(),
                    route.represented_subject.clone(),
                    application_operations::mcp_access_restrictions(
                        &native.grant_administrator,
                        &route.logical_executor,
                        &member.audience,
                        server,
                        ["read"],
                        &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
                    ),
                )
                .unwrap();
            let target = GrantTarget::McpServer {
                server: server.into(),
            };
            targets.push(TargetGrantLineage {
                target: target.clone(),
                lineage: vec![parent],
            });
            requests.push(operator_setup::SetupTargetRequest {
                target: Some(target),
                issuance_id: EvidenceId::new(format!("waiter-{server}")).unwrap(),
            });
        }
        let mut helper = acquisition::InstalledSetupSelection {
            catalog_capability: None,
            report_capability: None,
            member: member.member.clone(),
            ingress_actor: native.grant_administrator.clone(),
            logical_executor: route.logical_executor.clone(),
            represented_subject: route.represented_subject.clone(),
            controller_parent: route.controller_parent.clone().unwrap(),
            operation_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets,
            },
        };
        if public {
            helper.report_capability = Some(operator_setup::configured_report::issue_or_recover(
                &source,
                &native.grant_administrator,
                operator,
                &helper,
                &operator_setup::HelperReportCapabilityRequest {
                    member: member.member.clone(),
                    issuance_id: EvidenceId::new("workflow-origin-report-parent").unwrap(),
                    account_issuance_id: None,
                    public_issuance_id: Some(
                        EvidenceId::new("workflow-origin-public-parent").unwrap(),
                    ),
                },
                operator_setup::configured_report::report_parent(
                    &native.grant_administrator,
                    &route.logical_executor,
                    &member.audience,
                ),
                &member.audience,
            )?);
            requests.push(operator_setup::SetupTargetRequest {
                target: Some(acquisition::ConfiguredHelperReportCapability::public_target()),
                issuance_id: EvidenceId::new("waiter-public-service").unwrap(),
            });
            requests.sort_by(|left, right| left.target.cmp(&right.target));
        }
        let result = operator_setup::configured_setup::configure_shared_prepared(
            &native.preparation(),
            &policy.channels,
            &source,
            operator,
            &operator_setup::SharedWorkflowSetupRequest {
                member: native.members[0].member.clone(),
                route: native.routes[0].id.clone(),
                issuance_id: EvidenceId::new("effect-configured-acquisition-parent").unwrap(),
                helper: member.member.clone(),
                targets: requests,
            },
            &helper,
        );
        drop(originals);
        drop(source);
        result
    })
    .unwrap();
    let mut local: meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    local.native_host.as_mut().unwrap().members[0].setup_capability = Some(capability);
    super::tests::replace_local(&mut selection, &local);
    reopen_for_dedicated_workflow(directory, selection).await
}

pub(crate) async fn reopen_for_dedicated_workflow(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
) -> DedicatedHost {
    let (lifetime, installed) =
        InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
            .await
            .unwrap();
    let lifetime = Arc::new(lifetime);
    let source_path = state_directory(
        &selection,
        lifetime.selected_policy().native_host.as_ref().unwrap(),
        NativeOwnerStartup::Restore,
    )
    .unwrap()
    .join("originals.sqlite3");
    DedicatedHost {
        selection,
        store: installed.store.clone(),
        source_path,
        source_scope: installed.source_scope.clone(),
        directory,
        lifetime,
        originals: installed.originals(),
        installed: Some(installed),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "test fixture; each argument selects one fixture variant"
)]
async fn open_selected_source_with_workflow_members(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
    source_subject: bool,
    acquisition: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    helper: bool,
    report: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    configured_setup: bool,
    configured_report: bool,
    prepare_source: Option<PrepareOriginalSource>,
    console_human: Option<meerkat_core::PrincipalRef>,
    task_parent_administration: bool,
    private_helper_entry: bool,
    human_policy: bool,
    catalog_probe: bool,
    warm_profile: Option<WarmHelperProfile>,
    workflow_members: bool,
) -> Host {
    let (directory, selection, acquisition_helper, prepare_source) =
        provision_selected_source_with_workflow_members(
            facts,
            select_controller,
            probe,
            local_operator,
            source_subject,
            acquisition,
            helper,
            report,
            configured_setup,
            configured_report,
            prepare_source,
            console_human,
            task_parent_administration,
            private_helper_entry,
            human_policy,
            catalog_probe,
            warm_profile,
            workflow_members,
        );
    restore_selected(
        directory,
        selection,
        acquisition_helper,
        prepare_source,
        human_policy,
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "test fixture; each argument selects one fixture variant"
)]
fn provision_selected_source_with_workflow_members(
    facts: &ControllerModelFacts,
    select_controller: bool,
    probe: bool,
    local_operator: bool,
    source_subject: bool,
    acquisition: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    helper: bool,
    report: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    configured_setup: bool,
    configured_report: bool,
    prepare_source: Option<PrepareOriginalSource>,
    console_human: Option<meerkat_core::PrincipalRef>,
    task_parent_administration: bool,
    private_helper_entry: bool,
    human_policy: bool,
    catalog_probe: bool,
    warm_profile: Option<WarmHelperProfile>,
    workflow_members: bool,
) -> (
    Arc<tempfile::TempDir>,
    HostBootstrapSelection,
    Option<acquisition::InstalledSetupSelection>,
    Option<PrepareOriginalSource>,
) {
    assert!(
        !private_helper_entry
            || (helper && report.is_none() && !configured_setup && !configured_report)
    );
    let (directory, mut selection) = super::tests::fixture();
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let native = local.native_host.as_mut().unwrap();
    native.members[0].member.role = "worker".into();
    let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole {
        member,
        ..
    } = &mut native.routes[0].origin
    else {
        panic!("local fixture")
    };
    member.role = "worker".into();
    // This variant exercises the production local service review route. The
    // native root/requester/audience are selected before owner construction;
    // ordinary fixture modes retain their original synthetic administrator.
    let mut root = if local_operator {
        let operator =
            meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
                .unwrap()
                .operator;
        native.members[0].audience =
            meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                principal: operator.clone(),
            };
        let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole {
            requester,
            ..
        } = &mut native.routes[0].origin
        else {
            panic!("local fixture")
        };
        *requester = operator.clone();
        if source_subject {
            // An explicit fixture source selection, not authentication of a
            // Console login or a cast of its principal string.
            native.routes[0].represented_subject = Some(operator.clone());
        }
        operator
    } else {
        super::tests::principal("admin")
    };
    if let Some(human) = console_human {
        // Ordinary Console fixtures retain their existing separate root. The
        // explicit operator variant selects its root before initialization.
        if configured_setup {
            root = human.clone();
        }
        native.members[0].audience =
            meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                principal: human.clone(),
            };
        let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole {
            requester,
            ..
        } = &mut native.routes[0].origin
        else {
            panic!("local fixture")
        };
        *requester = human.clone();
        native.routes[0].represented_subject = Some(human);
    }
    if configured_setup || configured_report || task_parent_administration {
        native.remote_operator = Some(root.clone());
    }
    if task_parent_administration {
        native.service_parent_administration = Some(
            meerkat_toolkit::host_lifetime::policy::ServiceParentAdministration::ApplicationPolicyTasks {
                routes: [native.routes[0].id.clone()].into(),
            });
        assert!(native.routes[0].application_parent.is_none());
    }
    // The host explicitly selects the native comms package's five tool
    // identities for its initial artifact surface. Visibility is not an
    // operation grant, and the real registration hook checks built tools.
    let mut surface: Vec<_> = [
        "send_message",
        "reply_to_peer",
        "send_request",
        "send_response",
        "peers",
    ]
    .into_iter()
    .map(|name| meerkat_core::ToolIdentity {
        name: name.into(),
        provenance: Some(meerkat_core::ToolProvenance {
            kind: meerkat_core::ToolSourceKind::Comms,
            source_id: "comms".into(),
            artifact: None,
        }),
    })
    .collect();
    if probe {
        surface.push(super::acquisition::ContextProbe::definition().identity());
    }
    // These immutable leaf bundles have no deferred catalog owner. Native mob
    // profile mounting attributes an otherwise unowned leaf to its registered
    // Rust bundle before the installed-surface hook; preserve explicit owners.
    let bundle_identity = |tool: &meerkat_core::ToolDef, bundle: &str| {
        let mut identity = tool.identity();
        identity.provenance.get_or_insert_with(|| {
            meerkat_core::ToolProvenance::new(meerkat_core::ToolSourceKind::RustBundle, bundle)
        });
        identity
    };
    if let Some(tool) = acquisition {
        // Use the real bundle's catalog identities. This permits registration,
        // not operation access; the source still selects no application parent.
        use meerkat_core::AgentToolDispatcher;
        let registered: Arc<dyn AgentToolDispatcher> = Arc::new(
            meerkat_core::RegisteredToolBundleDispatcher::new(crate::FIND_AND_INSTALL_BUNDLE, tool),
        );
        surface.extend(
            registered
                .tools()
                .iter()
                .map(|tool| bundle_identity(tool, crate::FIND_AND_INSTALL_BUNDLE)),
        );
        // The governed factory also owns an initially empty MCP router. Its
        // dynamic exact catalog composes these native control tools when the
        // acquisition bundle preserves catalog exactness. Select their actual
        // identities too; this grants no catalog or setup operation authority.
        let catalog = meerkat_tools::CatalogControlDispatcher::new(
            registered,
            Arc::new(meerkat_tools::CatalogControlVisibilityProvider::new()),
        );
        surface.extend(catalog.tools().iter().map(|tool| tool.identity()));
    } else if !probe || human_policy || catalog_probe {
        // Plain, configured and exact catalog/human-policy fixtures compose native
        // catalog controls from their initially empty MCP router. Select the native
        // definitions in the artifact without granting operation authority.
        use meerkat_core::AgentToolDispatcher;
        let catalog = meerkat_tools::CatalogControlDispatcher::new(
            Arc::new(meerkat::McpRouter::new_with_stdio_profile(
                meerkat::McpStdioLaunchProfile::trusted_host(),
            )),
            Arc::new(meerkat_tools::CatalogControlVisibilityProvider::new()),
        );
        surface.extend(catalog.tools().iter().map(|tool| tool.identity()));
    }
    if let Some(tool) = &report {
        use meerkat_core::AgentToolDispatcher;
        let registered = meerkat_core::RegisteredToolBundleDispatcher::new(
            meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE,
            tool.clone(),
        );
        surface.extend(
            registered
                .tools()
                .iter()
                .map(|tool| bundle_identity(tool, meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE)),
        );
    }
    let surface_bytes = canonical_json(&surface).unwrap();
    let surface_artifact = &mut native.members[0].initial_surface;
    surface_artifact.sha256 = digest(&surface_bytes);
    fs::write(
        selection
            .local_policy
            .path
            .parent()
            .unwrap()
            .join(&surface_artifact.name),
        surface_bytes,
    )
    .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&selection.policy_manifest.path).unwrap()).unwrap();
    for row in manifest["artifacts"].as_array_mut().unwrap() {
        if row["path"] == surface_artifact.name {
            row["sha256"] = surface_artifact.sha256.clone().into();
        }
    }
    fs::write(
        &selection.policy_manifest.path,
        canonical_json(&manifest).unwrap(),
    )
    .unwrap();
    if workflow_members {
        assert!(!helper && !human_policy && !configured_report);
        for name in ["planning_parent", "planning_lead"] {
            let mut member = native.members[0].clone();
            member.member.member = name.into();
            member.logical_owner = super::tests::principal(name);
            let mut route = native.routes[0].clone();
            route.id = Id::new(name).unwrap();
            route.logical_executor = member.logical_owner.clone();
            let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole {
                member: target,
                ..
            } = &mut route.origin
            else {
                panic!("local fixture")
            };
            *target = member.member.clone();
            native.members.push(member);
            native.routes.push(route);
        }
    }
    if helper {
        let mut selected = native.members[0].clone();
        selected.member.member = HELPER.into();
        if report.is_some() {
            selected.member.role = REPORT_PROFILE.into();
        } else if private_helper_entry {
            selected.member.role = PRIVATE_HELPER_PROFILE.into();
        }
        selected.logical_owner = super::tests::principal(HELPER);
        // Select supported legacy host profiles before opening any owner. The
        // warm capability is optional; these profiles must still prepare the
        // existing public task without widening its metadata permission.
        match warm_profile {
            Some(WarmHelperProfile::SameExecutor) => {
                selected.logical_owner = native.members[0].logical_owner.clone()
            }
            Some(WarmHelperProfile::CrossAudience) => {
                selected.audience =
                    meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                        principal: selected.logical_owner.clone(),
                    }
            }
            _ => (),
        }
        native.members.push(selected);
    }
    if helper || human_policy || workflow_members {
        // Only A may invoke the installed acquisition leaf at the application
        // policy layer. Its separate native setup authority is still required;
        // ordinary AA retains an independent controller-only helper. The separate
        // report variant below selects only its helper report invocation.
        let tool = meerkat_core::CompiledApplicationToolPolicy::new(
            meerkat_core::PolicyProviderId::new("fixture").unwrap(),
            meerkat_core::PolicyId::new("fixture").unwrap(),
            meerkat_core::PolicyRevision(1),
            meerkat_core::CompiledPolicySourceProvenance {
                source_id: "fixture-source".into(),
                source_digest: meerkat_core::PolicyDigest::from_canonical_bytes(b"fixture"),
            },
            {
                let mut members: Vec<_> = native
                    .members
                    .iter()
                    .map(|member| meerkat_core::CompiledMemberToolGrants {
                        member_identity: member.member.member.clone(),
                        grants: if member.member.member == native.members[0].member.member {
                            vec![meerkat_core::CompiledMemberToolGrant {
                                tool_name: crate::FIND_AND_INSTALL.into(),
                                action: meerkat_core::CompiledMemberToolAction::Invoke,
                                consequence: meerkat_core::CompiledToolConsequence::R2,
                            }]
                        } else if catalog_probe
                            && (member.member.member == HELPER
                                || (workflow_members && member.member.member == "planning_lead"))
                        {
                            ["tool_catalog_load", "tool_catalog_search"]
                                .into_iter()
                                .map(|name| meerkat_core::CompiledMemberToolGrant {
                                    tool_name: name.into(),
                                    action: meerkat_core::CompiledMemberToolAction::Invoke,
                                    consequence: meerkat_core::CompiledToolConsequence::R0,
                                })
                                .collect()
                        } else if report.is_some() && member.member.member == HELPER {
                            // Fixture provisioning of only this helper's result
                            // operation. No account, installation or Ready access.
                            let mut grants = vec![meerkat_core::CompiledMemberToolGrant {
                                tool_name: meerkat_toolkit::setup::SETUP_REPORT.into(),
                                action: meerkat_core::CompiledMemberToolAction::Invoke,
                                consequence: meerkat_core::CompiledToolConsequence::R2,
                            }];
                            if human_policy && configured_report {
                                grants.extend(
                                    ["tool_catalog_load", "tool_catalog_search"]
                                        .into_iter()
                                        .map(|name| meerkat_core::CompiledMemberToolGrant {
                                            tool_name: name.into(),
                                            action: meerkat_core::CompiledMemberToolAction::Invoke,
                                            consequence: meerkat_core::CompiledToolConsequence::R0,
                                        }),
                                );
                            }
                            grants
                        } else {
                            vec![]
                        },
                    })
                    .collect();
                members.sort_by(|left, right| left.member_identity.cmp(&right.member_identity));
                members
            },
        )
        .unwrap();
        let bytes = tool.canonical_json().unwrap();
        native.application_tool_policy.sha256 = digest(&bytes);
        fs::write(
            selection
                .local_policy
                .path
                .parent()
                .unwrap()
                .join(&native.application_tool_policy.name),
            bytes,
        )
        .unwrap();
        for row in manifest["artifacts"].as_array_mut().unwrap() {
            if row["path"] == native.application_tool_policy.name {
                row["sha256"] = native.application_tool_policy.sha256.clone().into();
            }
        }
        fs::write(
            &selection.policy_manifest.path,
            canonical_json(&manifest).unwrap(),
        )
        .unwrap();
    }
    let mut acquisition_helper = None;
    let directory_path =
        state_directory(&selection, native, NativeOwnerStartup::Initialize).unwrap();
    {
        let source = ToolkitOriginalOwner::initialize(
            LocalGrantConfiguration {
                root: root.clone(),
                namespace: EvidenceId::new("controller-first-input").unwrap(),
                generation: 1,
            },
            Arc::new(FileGrantOwnerStore::new(
                directory_path.join("grants.jsonl"),
                meerkat_authorization::grants::GrantLogFence::NotAdmissionRead,
            )),
            Arc::new(HostAuthorizationClock),
            native.ingress_domain.clone(),
        )
        .unwrap();
        let _store = SqliteStore::open(directory_path.join("originals.sqlite3")).unwrap();
        if select_controller {
            let parent = source
                .grants()
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &root,
                    EvidenceId::new("controller-parent").unwrap(),
                    native.grant_administrator.clone(),
                    native.routes[0].represented_subject.clone(),
                    application_operations::parent_restrictions(
                        application_operations::controller(
                            &native.grant_administrator,
                            &native.routes[0].logical_executor,
                            &native.members[0].audience,
                            &facts.route_key(),
                            application_operations::ControllerAction::Inference,
                        ),
                    ),
                )
                .unwrap();
            native.routes[0].controller_parent = Some(vec![parent]);
        }
        if workflow_members {
            for index in 1..native.routes.len() {
                let route = &mut native.routes[index];
                let member = &native.members[index];
                let parent = source
                    .grants()
                    .issue_root(
                        &meerkat_core::AdmissionGuard::none(),
                        &root,
                        EvidenceId::new(format!("workflow-controller-parent-{index}")).unwrap(),
                        native.grant_administrator.clone(),
                        route.represented_subject.clone(),
                        application_operations::parent_restrictions(
                            application_operations::controller(
                                &native.grant_administrator,
                                &route.logical_executor,
                                &member.audience,
                                &facts.route_key(),
                                application_operations::ControllerAction::Inference,
                            ),
                        ),
                    )
                    .unwrap();
                route.controller_parent = Some(vec![parent]);
            }
            if configured_setup {
                let member = &native.members[2];
                let route = &native.routes[2];
                acquisition_helper = Some(acquisition::InstalledSetupSelection {
                    catalog_capability: None, report_capability: None,
                    member: member.member.clone(), ingress_actor: native.grant_administrator.clone(),
                    logical_executor: route.logical_executor.clone(), represented_subject: route.represented_subject.clone(),
                    controller_parent: route.controller_parent.clone().unwrap(),
                    operation_basis: meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages {
                        base: None, targets: vec![],
                    },
                });
            }
        }
        if helper {
            let member = &native.members[1];
            let parent = source
                .grants()
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &root,
                    EvidenceId::new("helper-controller-parent").unwrap(),
                    native.grant_administrator.clone(),
                    None,
                    application_operations::parent_restrictions(
                        application_operations::controller(
                            &native.grant_administrator,
                            &member.logical_owner,
                            &member.audience,
                            &facts.route_key(),
                            application_operations::ControllerAction::Inference,
                        ),
                    ),
                )
                .unwrap();
            acquisition_helper = Some(acquisition::InstalledSetupSelection {
                catalog_capability: None,
                report_capability: None,
                member: member.member.clone(), ingress_actor: native.grant_administrator.clone(),
                logical_executor: member.logical_owner.clone(), represented_subject: None,
                controller_parent: vec![parent],
                operation_basis: meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages {
                    base: None, targets: vec![],
                },
            });
        }
        if human_policy && configured_report {
            let helper = acquisition_helper.as_mut().unwrap();
            let member = native
                .members
                .iter()
                .find(|member| member.member == helper.member)
                .unwrap();
            let parent = source
                .grants()
                .issue_root(
                    &meerkat_core::AdmissionGuard::none(),
                    &root,
                    EvidenceId::new("warm-helper-catalog-parent").unwrap(),
                    native.grant_administrator.clone(),
                    helper.represented_subject.clone(),
                    application_operations::configured_catalog_parent(
                        &native.grant_administrator,
                        &helper.logical_executor,
                        &member.audience,
                    ),
                )
                .unwrap();
            helper.catalog_capability = Some(acquisition::ConfiguredHelperCatalogCapability {
                parent: vec![parent],
            });
        }
        assert!(native.routes[0].application_parent.is_none());
    }
    super::tests::replace_local(&mut selection, &local);
    if configured_report {
        use operator_setup::{HelperReportCapabilityRequest, OperatorSetup};
        let helper = acquisition_helper
            .as_mut()
            .expect("selected helper before runtime construction");
        assert!(helper.report_capability.is_none());
        assert!(matches!(&helper.operation_basis,
            meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages {
                base: None, targets,
            } if targets.is_empty()));
        let request = HelperReportCapabilityRequest {
            account_issuance_id: Some(EvidenceId::new("configured-helper-account-parent").unwrap()),
            public_issuance_id: Some(EvidenceId::new("configured-helper-public-parent").unwrap()),
            member: helper.member.clone(),
            issuance_id: EvidenceId::new("configured-helper-report-parent").unwrap(),
        };
        // Actual trusted producer, before a service or helper session exists.
        // The fixture supplies its selected helper projection; it does not
        // exercise the Connect artifact reader or Forge publication transaction.
        let selected = helper.clone();
        let capability =
            OperatorSetup::configure_application_helper_report(&selection, &request, |_| {
                Ok(selected.clone())
            })
            .unwrap();
        assert!(
            OperatorSetup::configure_application_helper_report(&selection, &request, |_| Ok(
                selected.clone()
            ),)
            .unwrap()
                == capability,
            "exact producer request recovers its native parent"
        );
        // Publish this receipt only into the existing typed helper selection.
        // Ordinary preparation issues the qualified-origin children later.
        helper.report_capability = Some(capability);
    }
    if configured_setup {
        use operator_setup::{AgentSetupCapabilityRequest, OperatorSetup};
        let native = local.native_host.as_ref().unwrap();
        let mut request = AgentSetupCapabilityRequest {
            helper: None,
            member: native.members[0].member.clone(),
            helper_targets: if configured_report {
                vec![
                    operator_setup::SetupTargetRequest {
                        target: Some(
                            acquisition::ConfiguredHelperReportCapability::public_target(),
                        ),
                        issuance_id: EvidenceId::new("configured-a-public-parent").unwrap(),
                    },
                    operator_setup::SetupTargetRequest {
                        target: Some(
                            acquisition::ConfiguredHelperReportCapability::account_target(),
                        ),
                        issuance_id: EvidenceId::new("configured-a-account-parent").unwrap(),
                    },
                    operator_setup::SetupTargetRequest {
                        target: Some(acquisition::ConfiguredHelperReportCapability::target()),
                        issuance_id: EvidenceId::new("configured-a-report-parent").unwrap(),
                    },
                ]
            } else {
                vec![]
            },
            route: native.routes[0].id.clone(),
            issuance_id: EvidenceId::new("configured-acquisition-parent").unwrap(),
        };
        if workflow_members {
            request.helper = acquisition_helper
                .as_ref()
                .map(|helper| helper.member.clone());
        }
        if human_policy && configured_report {
            request
                .helper_targets
                .push(operator_setup::SetupTargetRequest {
                    target: None,
                    issuance_id: EvidenceId::new("warm-original-catalog-parent").unwrap(),
                });
            request
                .helper_targets
                .sort_by(|left, right| left.target.cmp(&right.target));
        }
        // Actual trusted producer, same native owner restored under the stopped
        // candidate fence. Retry the same request to test native reconciliation.
        let selected_helper = acquisition_helper.clone();
        let configure = |request: &AgentSetupCapabilityRequest| {
            OperatorSetup::configure_application_agent_setup_with_helper(
                &selection,
                request,
                |_| Ok(selected_helper.clone()),
            )
        };
        let capability = configure(&request).unwrap();
        assert_eq!(configure(&request).unwrap(), capability);
        let mut wrong = request.clone();
        wrong.member.member = "unselected-agent".into();
        assert!(configure(&wrong).is_err());
        local.native_host.as_mut().unwrap().members[0].setup_capability = Some(capability);
        super::tests::replace_local(&mut selection, &local);
    }
    (
        Arc::new(directory),
        selection,
        acquisition_helper,
        prepare_source,
    )
}

/// Reopen the exact installed pair after both previous owners have settled and
/// dropped. This path restores existing grants and source rows; it issues none.
pub(crate) async fn reopen_for_continuation(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
) -> Host {
    restore_selected(directory, selection, None, None, false).await
}

pub(crate) async fn reopen_for_member_classes(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
    participant: Arc<dyn crate::private_originals::ApplicationServicePolicy>,
) -> Host {
    restore_selected_participant(directory, selection, None, None, Some(participant), None).await
}

pub(crate) async fn reopen_for_continuation_with_source(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
    source_store: PrepareOriginalSource,
) -> Host {
    restore_selected(directory, selection, None, Some(source_store), false).await
}

pub(crate) async fn reopen_for_helper_recovery(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
    helper: acquisition::InstalledSetupSelection,
    source_store: Option<PrepareOriginalSource>,
) -> Host {
    restore_selected(directory, selection, Some(helper), source_store, false).await
}

#[derive(Clone, Copy)]
pub(crate) enum ApplicationServiceSource {
    StockOperator,
    ApplicationService,
}

/// The persistent fixture has already created this actual session, joined
/// shutdown and dropped its owners. Provision an exact finite native parent
/// before restoring the same session. No guessed session or wildcard bounds.
pub(crate) fn configure_service_parent_before_restore(
    selection: &mut HostBootstrapSelection,
    session: &meerkat_core::SessionId,
    config: &meerkat_core::McpServerConfig,
    source_kind: ApplicationServiceSource,
) {
    use crate::private_originals::{
        PrivateAccessChoice, PrivateOriginalRoute, PrivateServiceMcpAccessDecision,
        PrivateServiceMcpLiveAttachment, PrivateServiceMcpPermission, PrivateSourceEvidence,
        ServiceMcpSelection,
    };
    use meerkat_authorization_contracts::constraints::DelegationDepth;
    use meerkat_authorization_contracts::work_association::NativeWorkTarget;
    use meerkat_toolkit::authority::{LifetimeScope, RoleLifetime};
    use toolkit_package::{Id, Revision};
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let native = local.native_host.as_mut().unwrap();
    assert!(native.routes[0].application_parent.is_none());
    if matches!(source_kind, ApplicationServiceSource::ApplicationService) {
        let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole {
            requester,
            ..
        } = &mut native.routes[0].origin
        else {
            panic!("existing local fixture route")
        };
        *requester = native.grant_administrator.clone();
        native.routes[0].represented_subject = None;
    }
    let selected = &native.routes[0];
    let (binding, requester, actor) = selected.source_binding_in(&local.channels).unwrap();
    let member = native
        .members
        .iter()
        .find(|member| &member.member == binding)
        .unwrap();
    let route = PrivateOriginalRoute {
        id: selected.id.clone(),
        grant_caller: native.grant_administrator.clone(),
        application_parent: None,
        controller_parent: selected.controller_parent.clone(),
        grant_target: Id::new(binding.member.clone()).unwrap(),
        grant_audience: member.audience.clone(),
        realm: native.realm.clone(),
        requester: requester.clone(),
        ingress_actor: actor.clone(),
        logical_executor: selected.logical_executor.clone(),
        represented_subject: selected.represented_subject.clone(),
        source_authority: selected.source_authority.clone(),
        authentication_domain: selected.authentication_domain.clone(),
        event_domain: selected.event_domain.clone(),
    };
    let destination = NativeWorkTarget {
        logical_owner: member.logical_owner.clone(),
        logical_runtime: EvidenceId::new(
            meerkat_runtime::LogicalRuntimeId::for_session(session).to_string(),
        )
        .unwrap(),
        context: EvidenceId::new(session.to_string()).unwrap(),
        context_generation: member.context_generation,
        audience: member.audience.clone(),
    };
    let directory = state_directory(selection, native, NativeOwnerStartup::Restore).unwrap();
    {
        let owner = ToolkitOriginalOwner::restore(
            Arc::new(FileGrantOwnerStore::new(
                directory.join("grants.jsonl"),
                meerkat_authorization::grants::GrantLogFence::NotAdmissionRead,
            )),
            Arc::new(HostAuthorizationClock),
            native.ingress_domain.clone(),
        )
        .unwrap();
        let now = owner.clock().now().unwrap().unix_ms;
        let decision = PrivateServiceMcpAccessDecision {
            catalog: None,
            route: route.id.clone(),
            decision: Id::new("stopped-service-parent").unwrap(),
            evidence: PrivateSourceEvidence {
                resource_id: EvidenceId::new("stopped-service-parent").unwrap(),
                revision: EvidenceId::new("fixture-v1").unwrap(),
                sha256: digest(b"stopped service fixture"),
            },
            choice: PrivateAccessChoice::Approve,
            permission: PrivateServiceMcpPermission {
                target: route.grant_target.clone(),
                connection: Id::new(&config.name).unwrap(),
                resource: Id::new(&config.name).unwrap(),
                operations: [Id::new("read").unwrap()].into(),
                lifetime: RoleLifetime {
                    scope: LifetimeScope::Task {
                        task: Id::new("stopped-service-parent").unwrap(),
                    },
                    expires_at_ms: Some(now.checked_add(300_000).unwrap()),
                },
            },
            connection_revision: Revision::new(digest(&canonical_json(config).unwrap())).unwrap(),
            live_attachment: Some(PrivateServiceMcpLiveAttachment { destination }),
        };
        let selected = match meerkat::ConnectorMcpInstallation::from_config(config) {
            Some(connector) => ServiceMcpSelection::connector(&connector.unwrap()),
            None => ServiceMcpSelection::no_connector(config).unwrap(),
        };
        let mut restrictions = selected.restrictions(&route, &decision, now).unwrap();
        restrictions.delegation_depth = DelegationDepth::remaining(1);
        let root =
            meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
                .unwrap()
                .operator;
        let parent = owner
            .grants()
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &root,
                EvidenceId::new("stopped-service-parent").unwrap(),
                native.grant_administrator.clone(),
                route.represented_subject.clone(),
                restrictions,
            )
            .unwrap();
        native.routes[0].application_parent = Some(vec![parent]);
    }
    super::tests::replace_local(selection, &local);
}

async fn restore_selected(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
    acquisition_helper: Option<acquisition::InstalledSetupSelection>,
    prepare_source: Option<PrepareOriginalSource>,
    human_policy: bool,
) -> Host {
    let participant = human_policy.then(|| human_policy_recovery::selected(&selection));
    restore_selected_participant(
        directory,
        selection,
        acquisition_helper,
        prepare_source,
        participant,
        None,
    )
    .await
}

async fn restore_selected_participant(
    directory: Arc<tempfile::TempDir>,
    selection: HostBootstrapSelection,
    acquisition_helper: Option<acquisition::InstalledSetupSelection>,
    prepare_source: Option<PrepareOriginalSource>,
    participant: Option<Arc<dyn crate::private_originals::ApplicationServicePolicy>>,
    prepare_policy: Option<PrepareOriginalSource>,
) -> Host {
    let (lifetime, installed) = if let Some(decorate) = prepare_policy {
        assert!(prepare_source.is_none());
        InstalledToolkitHost::open_with_member_policy_store(
            selection.clone(),
            NativeOwnerStartup::Restore,
            participant.clone().unwrap(),
            decorate,
        )
        .await
    } else {
        match prepare_source {
            Some(prepare_source) => {
                InstalledToolkitHost::open_with_source_store(
                    selection.clone(),
                    NativeOwnerStartup::Restore,
                    participant.clone(),
                    prepare_source,
                )
                .await
            }
            None => match &participant {
                Some(participant) => {
                    InstalledToolkitHost::open_with_application_service_policy(
                        selection.clone(),
                        NativeOwnerStartup::Restore,
                        None,
                        participant.clone(),
                    )
                    .await
                }
                None => {
                    InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore).await
                }
            },
        }
    }
    .unwrap();
    if let Some(participant) = &participant {
        assert!(
            installed
                .originals
                .service_tool_policy
                .get()
                .unwrap()
                .same_participant(participant),
            "the actual canonical policy owner retains this selected human recovery participant"
        );
    }
    let lifetime = Arc::new(lifetime);
    let store = installed.store.clone();
    let source_scope = installed.source_scope.clone();
    let originals = installed.originals;
    originals.bind_host_lifetime(&lifetime).unwrap();
    let tool_policy_registry = installed.tool_policy.registry();
    let tool_policy = installed.tool_policy.clone();
    let registration = Arc::new(InstalledMemberRegistration {
        originals: originals.clone(),
        owner: lifetime.clone(),
        native: installed.native.clone(),
        surfaces: installed.surfaces,
        tool_policy: installed.tool_policy,
        update_associations: None,
    });
    let policy = Arc::new(InstalledApplicationPolicy {
        operation_review_policy: None,
        originals: originals.clone(),
        lifetime: lifetime.clone(),
        native: installed.native,
        selected: lifetime.selected_policy().clone(),
    });
    let configuration = originals.configuration(policy.clone());
    let directory_path = state_directory(
        &selection,
        lifetime.selected_policy().native_host.as_ref().unwrap(),
        NativeOwnerStartup::Restore,
    )
    .unwrap();
    Host {
        selection,
        store,
        source_path: directory_path.join("originals.sqlite3"),
        source_scope,
        tool_policy_registry,
        tool_policy,
        directory,
        lifetime,
        originals,
        configuration,
        policy,
        registration,
        acquisition_helper,
    }
}

/// Retain the real installed acquisition and its immutable leaves. This denial
/// fixture deliberately does not claim a ready continuation/helper runtime.
/// Native operation refusal must happen before either dependency is used.
pub(crate) fn bind_acquisition(
    host: &Host,
    tools: acquisition::InstalledAcquisitionTools,
) -> Arc<acquisition::InstalledAcquisition> {
    bind_acquisition_parts(
        &host.store,
        &host.source_scope,
        &host.originals,
        &host.lifetime,
        &host.source_path,
        tools,
        None,
    )
}

/// Same installed owner, selected only by the workflow worker control. No
/// helper dispatch is exercised; the actual recovery worker consumes close.
pub(crate) fn bind_workflow_recovery(
    store: &Arc<toolkit_store::SqliteStore>,
    source_scope: &toolkit_store::StoreScope,
    originals: &Arc<crate::private_originals::PrivateOriginals>,
    lifetime: &Arc<NativeHostLifetime>,
    source_path: &std::path::Path,
) -> Arc<acquisition::InstalledAcquisition> {
    let timeout = acquisition::InstalledSetupLimits::default().admission_timeout_ms;
    let tools =
        acquisition::InstalledAcquisitionTools::new(std::time::Duration::from_millis(timeout))
            .unwrap();
    bind_acquisition_parts(
        store,
        source_scope,
        originals,
        lifetime,
        source_path,
        tools,
        None,
    )
}

/// Lifecycle-only continuation port. Native source/G ownership is real; the
/// supplied delivery owner is explicitly a protocol fixture, not native input.
pub(crate) fn bind_workflow_lifecycle_recovery(
    store: &Arc<toolkit_store::SqliteStore>,
    source_scope: &toolkit_store::StoreScope,
    originals: &Arc<crate::private_originals::PrivateOriginals>,
    lifetime: &Arc<NativeHostLifetime>,
    source_path: &std::path::Path,
    continuation: Arc<dyn acquisition::InstalledOriginalTaskOwner>,
) -> Arc<acquisition::InstalledAcquisition> {
    let timeout = acquisition::InstalledSetupLimits::default().admission_timeout_ms;
    let tools =
        acquisition::InstalledAcquisitionTools::new(std::time::Duration::from_millis(timeout))
            .unwrap();
    bind_acquisition_parts(
        store,
        source_scope,
        originals,
        lifetime,
        source_path,
        tools,
        Some(continuation),
    )
}

fn bind_acquisition_parts(
    store: &Arc<toolkit_store::SqliteStore>,
    source_scope: &toolkit_store::StoreScope,
    originals: &Arc<crate::private_originals::PrivateOriginals>,
    lifetime: &Arc<NativeHostLifetime>,
    source_path: &std::path::Path,
    tools: acquisition::InstalledAcquisitionTools,
    selected_continuation: Option<Arc<dyn acquisition::InstalledOriginalTaskOwner>>,
) -> Arc<acquisition::InstalledAcquisition> {
    use acquisition::{
        InstalledAcquisition, InstalledNativeContinuation, InstalledSetupDefaults,
        InstalledSetupLimits, InstalledSetupSelection,
    };
    use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
    use meerkat_core::{
        service::WorkGraphNamespaceGrant,
        storage_durability::{DurabilityDeclaration, DurabilityResolution},
    };
    use meerkat_toolkit::{
        authority::Ceiling,
        setup::{SetupWorkBackend, WorkAdmissionStepOptions},
    };
    use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphService};
    let id = |value: &str| toolkit_package::Id::new(value).unwrap();
    let policy = lifetime.selected_policy();
    let native = policy.native_host.as_ref().unwrap();
    let continuation = selected_continuation.unwrap_or_else(|| {
        InstalledNativeContinuation::new(
            store.clone(),
            source_scope.clone(),
            originals.clone(),
            id("controller-only-denial"),
            [id("person")].into(),
        )
        .unwrap()
    });
    let workgraph = WorkGraphService::with_namespace_grant(
        Arc::new(
            SqliteWorkGraphStore::open(source_path.with_file_name("denial-workgraph.sqlite"))
                .unwrap(),
        ),
        WorkGraphNamespaceGrant::new(native.realm.as_str(), "setup").unwrap(),
    )
    .unwrap();
    let work_backend = SetupWorkBackend::new(
        workgraph,
        "fixture".into(),
        id("generation"),
        &[DurabilityDeclaration::durable(
            "workgraph",
            DurabilityResolution::Persistent,
        )],
    )
    .unwrap();
    let mut helper = InstalledSetupSelection {
        catalog_capability: None,
        report_capability: None,
        member: native.members[0].member.clone(),
        ingress_actor: native.grant_administrator.clone(),
        logical_executor: native.members[0].logical_owner.clone(),
        represented_subject: None,
        controller_parent: native.routes[0].controller_parent.clone().unwrap(),
        operation_basis: WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![],
        },
    };
    if let Some(binding) = native.members[0]
        .setup_capability
        .as_ref()
        .and_then(|value| value.helper.as_ref())
    {
        // A configured workflow already selects its helper. Preserve that
        // exact member and route instead of substituting the original caller.
        // This fixture still supplies no helper operation authority or entry.
        let mut members = native
            .members
            .iter()
            .filter(|member| &member.member == binding);
        let member = members
            .next()
            .expect("configured helper is a compiled member");
        assert!(
            members.next().is_none(),
            "configured helper member is unique"
        );
        let mut routes = native.routes.iter().filter_map(|route| {
            let (member, _, actor) = route.source_binding_in(&policy.channels).ok()?;
            (member == binding).then_some((route, actor))
        });
        let (route, actor) = routes
            .next()
            .expect("configured helper has a compiled source route");
        assert!(routes.next().is_none(), "configured helper route is unique");
        helper.member = member.member.clone();
        helper.ingress_actor = actor.clone();
        helper.logical_executor = route.logical_executor.clone();
        helper.represented_subject = route.represented_subject.clone();
        helper.controller_parent = route.controller_parent.clone().unwrap();
    }
    let owner = Arc::new(
        InstalledAcquisition::from_defaults_with_tools(
            store.clone(),
            source_scope.clone(),
            originals.clone(),
            native,
            InstalledSetupDefaults {
                helper,
                proposal_ceiling: Ceiling {
                    actions: Default::default(),
                    services: Default::default(),
                    frontends: Default::default(),
                    output_audiences: [id("person")].into(),
                },
                continuation,
                work_backend,
                references: None,
                limits: InstalledSetupLimits::default(),
                work_step: WorkAdmissionStepOptions {
                    worker: id("worker"),
                    lease_ms: 100,
                    owner_timeout_ms: 25,
                    settlement_budget_ms: 25,
                },
            },
            tools,
        )
        .unwrap(),
    );
    owner.bind_tool_driver().unwrap();
    owner
}

// The positive acquisition fixture selects an independent controller-only
// helper before constructing the installed source/policy owner.
pub(crate) const HELPER: &str = "setup-helper";
pub(crate) const PRIVATE_HELPER_PROFILE: &str = "setup-entry-private";
pub(crate) async fn open_for_acquisition(
    facts: &ControllerModelFacts,
    tool: Arc<dyn meerkat_core::AgentToolDispatcher>,
    private_helper_entry: bool,
    catalog_probe: bool,
) -> Host {
    open_selected_source(
        facts,
        true,
        false,
        true,
        false,
        Some(tool),
        true,
        None,
        false,
        false,
        None,
        None,
        false,
        private_helper_entry,
        false,
        catalog_probe,
    )
    .await
}

pub(crate) async fn open_for_helper_recovery(
    facts: &ControllerModelFacts,
    tool: Arc<dyn meerkat_core::AgentToolDispatcher>,
    source_store: Option<PrepareOriginalSource>,
) -> Host {
    open_selected_source(
        facts,
        true,
        false,
        true,
        false,
        Some(tool),
        true,
        None,
        false,
        false,
        source_store,
        None,
        false,
        true,
        false,
        false,
    )
    .await
}
#[path = "controller_first_input_acquisition_fixture.rs"]
mod approved_acquisition;
pub(crate) use approved_acquisition::prepare_approved_acquisition;
pub(crate) const REPORT_PROFILE: &str = "setup-report-only";
pub(crate) async fn open_for_helper_report(
    facts: &ControllerModelFacts,
    acquisition: Arc<dyn meerkat_core::AgentToolDispatcher>,
    report: Arc<dyn meerkat_core::AgentToolDispatcher>,
) -> Host {
    open_selected_subject(
        facts,
        true,
        false,
        true,
        false,
        Some(acquisition),
        true,
        Some(report),
        false,
        false,
    )
    .await
}

/// Uses actual configured capability issuance before immutable host selection.
pub(crate) async fn open_with_configured_setup(facts: &ControllerModelFacts) -> Host {
    open_selected_subject(
        facts, true, false, true, false, None, false, None, true, false,
    )
    .await
}

pub(crate) async fn open_with_configured_setup_probe(facts: &ControllerModelFacts) -> Host {
    open_selected_subject(
        facts, true, true, true, false, None, false, None, true, false,
    )
    .await
}

pub(crate) async fn open_with_configured_setup_probe_source(
    facts: &ControllerModelFacts,
    prepare_source: PrepareOriginalSource,
) -> Host {
    open_selected_subject_with_source(
        facts,
        true,
        true,
        true,
        false,
        None,
        false,
        None,
        true,
        false,
        Some(prepare_source),
    )
    .await
}

pub(crate) async fn open_with_configured_setup_source(
    facts: &ControllerModelFacts,
    prepare_source: PrepareOriginalSource,
) -> Host {
    open_selected_subject_with_source(
        facts,
        true,
        false,
        true,
        false,
        None,
        false,
        None,
        true,
        false,
        Some(prepare_source),
    )
    .await
}

/// Separate report-parent mode. The existing direct-child report fixture is
/// unchanged; this mode selects only the trusted producer's parent receipt.
pub(crate) async fn open_for_configured_helper_report(
    facts: &ControllerModelFacts,
    acquisition: Arc<dyn meerkat_core::AgentToolDispatcher>,
    report: Arc<dyn meerkat_core::AgentToolDispatcher>,
) -> Host {
    open_selected_subject(
        facts,
        true,
        false,
        true,
        false,
        Some(acquisition),
        true,
        Some(report),
        true,
        true,
    )
    .await
}

pub(crate) async fn open_for_warm_helper_profile(
    facts: &ControllerModelFacts,
    acquisition: Arc<dyn meerkat_core::AgentToolDispatcher>,
    report: Arc<dyn meerkat_core::AgentToolDispatcher>,
    profile: WarmHelperProfile,
) -> Host {
    open_selected_source_with_profile(
        facts,
        true,
        false,
        true,
        false,
        Some(acquisition),
        true,
        Some(report),
        true,
        true,
        None,
        None,
        false,
        false,
        true,
        false,
        Some(profile),
    )
    .await
}

// Mirrors Connect's selected_policy::exact_human_policy_decision contract at
// this lower crate boundary; depending on meerkat-connect would be cyclic.
// The selection is immutable. Host/task currentness and canonical slot CAS
// remain the existing production owners' responsibility.
mod human_policy_recovery {
    use super::*;
    use crate::private_originals::{
        ActiveMemberToolPolicyDecision, ApplicationServicePolicy, ApplicationServicePolicyDecision,
        ApplicationServicePolicyRequest, ApplicationServicePolicyReservation,
        ApplicationToolPolicyRecoveryDecision, ApplicationToolPolicyRecoveryRequest,
        MemberPolicyLifetime,
    };
    use meerkat_core::{
        ApprovalDecision, ApprovalStatus, CompiledApplicationToolPolicy, PolicyDigest,
    };

    struct Selected {
        owner: meerkat_core::PrincipalRef,
        realm: String,
        deployment: String,
        bootstrap: CompiledApplicationToolPolicy,
    }
    pub(super) fn selected(
        selection: &HostBootstrapSelection,
    ) -> Arc<dyn ApplicationServicePolicy> {
        let local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let native = local.native_host.as_ref().unwrap();
        let bytes = read_bundle_artifact(selection, &native.application_tool_policy).unwrap();
        let bootstrap = CompiledApplicationToolPolicy::parse_canonical_json(&bytes).unwrap();
        // Check the actual immutable bootstrap artifact before any review.
        let original = meerkat_core::CompiledMemberToolGrants {
            member_identity: native.members[0].member.member.clone(),
            grants: vec![meerkat_core::CompiledMemberToolGrant {
                tool_name: crate::FIND_AND_INSTALL.into(),
                action: meerkat_core::CompiledMemberToolAction::Invoke,
                consequence: meerkat_core::CompiledToolConsequence::R2,
            }],
        };
        if native.members.len() == 1 {
            assert_eq!(
                bootstrap.members,
                vec![original],
                "captured A starts with no catalog or MCP invocation permission"
            );
        } else {
            assert_eq!(native.members.len(), 2);
            assert_eq!(native.members[1].member.member, HELPER);
            assert!(
                native.members[0]
                    .setup_capability
                    .as_ref()
                    .unwrap()
                    .helper_targets
                    .iter()
                    .any(|target| target.target.is_none())
            );
            let helper = meerkat_core::CompiledMemberToolGrants {
                member_identity: HELPER.into(),
                grants: vec![
                    meerkat_core::CompiledMemberToolGrant {
                        tool_name: meerkat_toolkit::setup::SETUP_REPORT.into(),
                        action: meerkat_core::CompiledMemberToolAction::Invoke,
                        consequence: meerkat_core::CompiledToolConsequence::R2,
                    },
                    meerkat_core::CompiledMemberToolGrant {
                        tool_name: "tool_catalog_load".into(),
                        action: meerkat_core::CompiledMemberToolAction::Invoke,
                        consequence: meerkat_core::CompiledToolConsequence::R0,
                    },
                    meerkat_core::CompiledMemberToolGrant {
                        tool_name: "tool_catalog_search".into(),
                        action: meerkat_core::CompiledMemberToolAction::Invoke,
                        consequence: meerkat_core::CompiledToolConsequence::R0,
                    },
                ],
            };
            let mut expected = vec![original, helper];
            expected.sort_by(|a, b| a.member_identity.cmp(&b.member_identity));
            for member in &mut expected {
                member.grants.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
            }
            assert_eq!(
                bootstrap.members, expected,
                "only this fixture's exact helper catalog/report policy is added"
            );
        }
        Arc::new(Selected {
            owner: native.grant_administrator.clone(),
            realm: native.realm.to_string(),
            deployment: local.deployment.to_string(),
            bootstrap,
        })
    }
    impl Selected {
        fn check(
            &self,
            decision: &ActiveMemberToolPolicyDecision,
            canonical: &[u8],
        ) -> Result<(), HostLifetimeError> {
            let proposal = &decision.proposal;
            let key = meerkat_toolkit::setup::SetupScope::new(
                toolkit_package::Id::new(&self.realm)
                    .map_err(|_| HostLifetimeError::InvalidSelection)?,
                toolkit_package::Id::new(&self.deployment)
                    .map_err(|_| HostLifetimeError::InvalidSelection)?,
            )
            .key(
                &toolkit_package::Id::new(&decision.acquisition.id)
                    .map_err(|_| HostLifetimeError::InvalidSelection)?,
            );
            let candidate = CompiledApplicationToolPolicy::parse_canonical_json(canonical)
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            // The real ServicePolicyOwner checks the selected current prior
            // under its canonical slot CAS. This participant authenticates the
            // exact approved successor, including later successors of bootstrap.
            let same_members = candidate
                .members
                .iter()
                .map(|member| &member.member_identity)
                .collect::<Vec<_>>()
                == self
                    .bootstrap
                    .members
                    .iter()
                    .map(|member| &member.member_identity)
                    .collect::<Vec<_>>();
            if decision.owner != self.owner
                || decision.realm != self.realm
                || decision.deployment != self.deployment
                || decision.acquisition != key
                || canonical != proposal.candidate_canonical_json.as_bytes()
                || proposal.lifetime != MemberPolicyLifetime::UntilSuperseded
                || proposal.provider_id != self.bootstrap.provider_id
                || proposal.policy_id != self.bootstrap.policy_id
                || candidate.provider_id != proposal.provider_id
                || candidate.policy_id != proposal.policy_id
                || candidate.revision != proposal.candidate.revision
                || PolicyDigest::from_canonical_bytes(canonical) != proposal.candidate.digest
                || !same_members
                || !candidate
                    .members
                    .iter()
                    .any(|member| member.member_identity == proposal.member)
                || decision.approval_id != proposal.sha256()?
            {
                return Err(HostLifetimeError::Denied);
            }
            let record = decision
                .human_review
                .as_ref()
                .ok_or(HostLifetimeError::Denied)?;
            let expected =
                serde_json::to_value(proposal).map_err(|_| HostLifetimeError::InvalidSelection)?;
            if record.status != ApprovalStatus::Approved
                || !record
                    .decision
                    .as_ref()
                    .is_some_and(|answer| answer.decision == ApprovalDecision::Approve)
                || record
                    .proposed_action
                    .body
                    .as_ref()
                    .and_then(|body| body.get("access_request"))
                    .and_then(|body| body.get("member_tool_policy"))
                    .and_then(|body| body.get("proposal"))
                    != Some(&expected)
            {
                return Err(HostLifetimeError::Denied);
            }
            Ok(())
        }
    }
    impl ApplicationServicePolicy for Selected {
        fn reserve(
            &self,
            _: &ApplicationServicePolicyRequest<'_>,
        ) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
            Ok(ApplicationServicePolicyDecision::RequiresHuman)
        }
        fn reserve_tool_policy_recovery(
            &self,
            request: &ApplicationToolPolicyRecoveryRequest<'_>,
        ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
            self.check(request.decision, request.canonical_json)?;
            // The exact immutable owner selection and approved whole candidate
            // are retained for every later currentness check, not a Permit bit.
            Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
                Reserved {
                    selected: Selected {
                        owner: self.owner.clone(),
                        realm: self.realm.clone(),
                        deployment: self.deployment.clone(),
                        bootstrap: self.bootstrap.clone(),
                    },
                    decision: request.decision.clone(),
                    canonical: request.canonical_json.to_vec(),
                    provenance: [
                        (
                            "fixture_policy".into(),
                            "exact-human-reviewed-candidate".into(),
                        ),
                        (
                            "proposal_sha256".into(),
                            request.decision.approval_id.clone().into(),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                },
            )))
        }
    }
    struct Reserved {
        selected: Selected,
        decision: ActiveMemberToolPolicyDecision,
        canonical: Vec<u8>,
        provenance: serde_json::Map<String, serde_json::Value>,
    }
    impl ApplicationServicePolicyReservation for Reserved {
        fn owner(&self) -> &meerkat_core::PrincipalRef {
            &self.selected.owner
        }
        fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> {
            &self.provenance
        }
        fn check_current(&self) -> Result<(), HostLifetimeError> {
            self.selected.check(&self.decision, &self.canonical)
        }
    }
}

/// The ordinary coordinator remains selected while a finite class has no
/// initial instances. Only the real native add creates the later instance.
pub(crate) async fn open_for_member_classes(
    facts: &ControllerModelFacts,
    class: meerkat_toolkit::host_lifetime::policy::CompiledMemberClass,
    participant: Arc<dyn crate::private_originals::ApplicationServicePolicy>,
    prepare_policy: Option<PrepareOriginalSource>,
    prepare_source: Option<PrepareOriginalSource>,
) -> Host {
    let (directory, mut selection, helper, source) =
        provision_selected_source_with_workflow_members(
            facts, true, false, true, false, None, false, None, false, false, None, None, false,
            false, false, false, None, false,
        );
    let mut local: CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
    let native = local.native_host.as_mut().unwrap();
    native.member_classes = vec![class];
    // Held completion issues the new member's controller parent through the
    // same actual local operator that provisioned this fixture's grant store.
    let operator =
        meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
            .unwrap()
            .operator;
    assert_eq!(
        native.routes[0].controller_parent.as_ref().unwrap()[0].root_authority,
        operator
    );
    native.remote_operator = Some(operator);
    native.authentication_domain = Some(
        meerkat_authorization_contracts::constraints::ResourceDomain {
            authority: native.ingress_domain.authority.clone(),
            namespace: "factory-console-authentication".into(),
        },
    );
    native.event_domain = Some(
        meerkat_authorization_contracts::constraints::ResourceDomain {
            authority: native.ingress_domain.authority.clone(),
            namespace: "factory-console-events".into(),
        },
    );
    for route in &mut native.routes {
        if matches!(
            &route.origin,
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { .. }
        ) {
            route.authentication_domain = native.authentication_domain.clone().unwrap();
            route.event_domain = native.event_domain.clone().unwrap();
        }
    }
    super::tests::replace_local(&mut selection, &local);
    assert!(source.is_none());
    restore_selected_participant(
        directory,
        selection,
        helper,
        prepare_source,
        Some(participant),
        prepare_policy,
    )
    .await
}
