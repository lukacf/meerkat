//! Real stopped-host issuer/native-store controls. The helper projection here
//! is fixture configuration; the production command reads the selected artifact.
use super::*;
use super::super::configured_report::{report_parent, HelperReportCapabilityRequest};
use crate::installed_host::acquisition::{ConfiguredHelperReportCapability, InstalledSetupSelection};
use meerkat_authorization_contracts::{constraints::AudienceRef, work_association::WorkAuthorityBasis};

struct Fixture {
    _directory: tempfile::TempDir,
    selection: HostBootstrapSelection,
    helper: InstalledSetupSelection,
    request: HelperReportCapabilityRequest,
    grants_path: std::path::PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let (directory, selection) = application_candidate();
        OperatorSetup::provision_application_candidate(&selection, "report-parent-owner", 1).unwrap();
        let (helper, grants_path) = NativeHostLifetime::with_candidate_installation(&selection, |policy| {
            let native = policy.native_host.as_ref().unwrap();
            let operator = native.remote_operator.as_ref().unwrap();
            let (source, originals) = open_owner_pair(&selection, native, operator, OperatorOwnerStartup::Restore)?;
            let controller = source.grants().issue_root(operator, EvidenceId::new("separate-controller-parent").unwrap(),
                native.grant_administrator.clone(), None, ExecutionRestrictions {
                    delegation_depth: DelegationDepth::remaining(1), ..ExecutionRestrictions::unrestricted()
                }).unwrap();
            let helper = InstalledSetupSelection {
                catalog_capability: None,
                report_capability: None, member: native.members[0].member.clone(),
                ingress_actor: super::super::super::tests::principal("helper-ingress"),
                logical_executor: native.members[0].logical_owner.clone(), represented_subject: None,
                controller_parent: vec![controller],
                operation_basis: WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] },
            };
            let grants_path = state_directory(&selection, native, NativeOwnerStartup::Restore)?.join("grants.jsonl");
            drop(originals); drop(source);
            Ok((helper, grants_path))
        }).unwrap();
        let request = HelperReportCapabilityRequest {
            account_issuance_id: None, public_issuance_id: None,
            member: helper.member.clone(), issuance_id: EvidenceId::new("exact-helper-report-parent").unwrap(),
        };
        Self { _directory: directory, selection, helper, request, grants_path }
    }
    fn configure(&self, helper: InstalledSetupSelection, request: &HelperReportCapabilityRequest)
        -> Result<ConfiguredHelperReportCapability, HostLifetimeError>
    {
        OperatorSetup::configure_application_helper_report(&self.selection, request, |_| Ok(helper))
    }
    fn bytes(&self) -> Vec<u8> { fs::read(&self.grants_path).unwrap() }
}

#[test]
fn report_parent_bootstraps_absent_capability_and_recovers_without_publication() {
    let fixture = Fixture::new();
    assert!(fixture.helper.report_capability.is_none());
    let helper_before = canonical_json(&fixture.helper).unwrap();
    let policy_before = fs::read(&fixture.selection.local_policy.path).unwrap();
    let manifest_before = fs::read(&fixture.selection.policy_manifest.path).unwrap();
    let current = fs::read_link(fixture.selection.deployment_root.join("current")).unwrap();
    let capability = fixture.configure(fixture.helper.clone(), &fixture.request).unwrap();
    let granted = fixture.bytes();
    assert!(fixture.configure(fixture.helper.clone(), &fixture.request).unwrap() == capability);
    assert_eq!(fixture.bytes(), granted);
    assert_eq!(canonical_json(&fixture.helper).unwrap(), helper_before);
    assert_eq!(fs::read(&fixture.selection.local_policy.path).unwrap(), policy_before);
    assert_eq!(fs::read(&fixture.selection.policy_manifest.path).unwrap(), manifest_before);
    assert_eq!(fs::read_link(fixture.selection.deployment_root.join("current")).unwrap(), current);
    NativeHostLifetime::with_candidate_installation(&fixture.selection, |policy| {
        let native = policy.native_host.as_ref().unwrap();
        let operator = native.remote_operator.as_ref().unwrap();
        let (source, originals) = open_owner_pair(&fixture.selection, native, operator, OperatorOwnerStartup::Restore)?;
        assert_eq!(capability.parent.len(), 1);
        assert_eq!(capability.parent[0].root_authority, *operator);
        let resolved = source.grants().resolve_controller_lineage(&capability.parent, &native.grant_administrator, None).unwrap();
        assert_eq!(resolved.restrictions(), &report_parent(&native.grant_administrator, &fixture.helper.logical_executor, &native.members[0].audience));
        resolved.check_current().unwrap();
        drop(resolved); drop(originals); drop(source);
        Ok(())
    }).unwrap();
}

#[test]
fn report_parent_recovery_refuses_changed_member_executor_audience_or_subject() {
    let mut fixture = Fixture::new();
    let capability = fixture.configure(fixture.helper.clone(), &fixture.request).unwrap();
    let granted = fixture.bytes();
    let mut wrong_request = fixture.request.clone();
    wrong_request.member.member = "different-member".into();
    assert!(matches!(fixture.configure(fixture.helper.clone(), &wrong_request), Err(HostLifetimeError::Denied)));
    let mut executor = fixture.helper.clone();
    executor.logical_executor = super::super::super::tests::principal("different-executor");
    let mut subject = fixture.helper.clone();
    subject.represented_subject = Some(super::super::super::tests::principal("different-subject"));
    for helper in [executor, subject] {
        assert!(matches!(fixture.configure(helper, &fixture.request), Err(HostLifetimeError::Denied)));
        assert_eq!(fixture.bytes(), granted);
    }
    // A newly selected compiled audience cannot reuse the old grant reference.
    let mut policy: meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy =
        serde_json::from_slice(&fs::read(&fixture.selection.local_policy.path).unwrap()).unwrap();
    policy.native_host.as_mut().unwrap().members[0].audience = AudienceRef::Principal {
        principal: super::super::super::tests::principal("different-audience"),
    };
    super::super::super::tests::replace_local(&mut fixture.selection, &policy);
    assert!(matches!(fixture.configure(fixture.helper.clone(), &fixture.request), Err(HostLifetimeError::Denied)));
    assert_eq!(fixture.bytes(), granted);
    assert_eq!(capability.parent[0].grant_id, fixture.request.issuance_id);
}

#[test]
fn report_parent_refuses_incompatible_existing_parent_base_and_missing_projection() {
    let fixture = Fixture::new();
    // Reusing an actual inference/controller parent's ID must not reinterpret it
    // as report permission or append a replacement parent.
    let mut wrong = fixture.request.clone();
    wrong.issuance_id = fixture.helper.controller_parent[0].grant_id.clone();
    let before = fixture.bytes();
    assert!(matches!(fixture.configure(fixture.helper.clone(), &wrong), Err(HostLifetimeError::Denied)));
    let mut occupied = fixture.helper.clone();
    occupied.operation_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: None, targets: vec![meerkat_authorization_contracts::work_association::TargetGrantLineage {
            target: ConfiguredHelperReportCapability::target(), lineage: occupied.controller_parent.clone(),
        }],
    };
    assert!(matches!(fixture.configure(occupied, &fixture.request), Err(HostLifetimeError::Denied)));
    assert!(matches!(OperatorSetup::configure_application_helper_report(&fixture.selection, &fixture.request,
        |_| Err(HostLifetimeError::Unavailable)), Err(HostLifetimeError::Unavailable)));
    assert_eq!(fixture.bytes(), before);
}


#[test]
fn configured_original_targets_recover_partial_roots_and_keep_helper_roots_independent() {
    let mut fixture = Fixture::new();
    fixture.request.account_issuance_id = Some(EvidenceId::new("exact-helper-account-parent").unwrap());
    let mut helper = fixture.helper.clone();
    helper.report_capability = Some(fixture.configure(helper.clone(), &fixture.request).unwrap());
    let mut request = NativeHostLifetime::with_candidate_installation(&fixture.selection, |policy| {
        let native = policy.native_host.as_ref().unwrap();
        Ok(AgentSetupCapabilityRequest {
            helper: None, member: native.members[0].member.clone(), route: native.routes[0].id.clone(),
            issuance_id: EvidenceId::new("original-target-find-parent").unwrap(), helper_targets: vec![] })
    }).unwrap();
    // A real first root exists before the paired request is resumed. Reusing
    // its retained ID must not append a replacement or manufacture a receipt.
    let first = OperatorSetup::configure_application_agent_setup(&fixture.selection, &request).unwrap();
    request.helper_targets = vec![
        SetupTargetRequest { target: Some(ConfiguredHelperReportCapability::account_target()),
            issuance_id: EvidenceId::new("original-target-account-parent").unwrap() },
        SetupTargetRequest { target: Some(ConfiguredHelperReportCapability::target()),
            issuance_id: EvidenceId::new("original-target-report-parent").unwrap() },
    ];
    let configure = |request: &AgentSetupCapabilityRequest, selected: InstalledSetupSelection| {
        OperatorSetup::configure_application_agent_setup_with_helper(&fixture.selection, request, |_| Ok(Some(selected)))
    };
    let selected = configure(&request, helper.clone()).unwrap();
    assert_eq!(selected.parent, first.parent);
    let before = fixture.bytes();
    assert_eq!(configure(&request, helper.clone()).unwrap(), selected);
    assert_eq!(fixture.bytes(), before);
    for target in &selected.helper_targets {
        assert_ne!(target.parent, selected.parent);
        assert!(!helper.report_capability.as_ref().unwrap().parent.contains(&target.parent));
        assert!(!helper.report_capability.as_ref().unwrap().account_parent.as_ref().unwrap().contains(&target.parent));
    }
    let mut wrong = request.clone(); wrong.helper_targets[1].issuance_id = wrong.helper_targets[0].issuance_id.clone();
    assert!(configure(&wrong, helper.clone()).is_err());
    let mut changed = helper.clone(); changed.logical_executor = super::super::super::tests::principal("wrong-helper");
    assert!(configure(&request, changed).is_err());
    assert_eq!(fixture.bytes(), before);
    NativeHostLifetime::with_candidate_installation(&fixture.selection, |policy| {
        let native = policy.native_host.as_ref().unwrap(); let operator = native.remote_operator.as_ref().unwrap();
        let (source, originals) = open_owner_pair(&fixture.selection, native, operator, OperatorOwnerStartup::Restore)?;
        let machine = crate::original_ingress::tests::machine(&source);
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        source.grants().revoke(operator, &selected.helper_targets[0].parent, &mut custody).unwrap();
        assert!(source.grants().resolve_lineage(std::slice::from_ref(&selected.helper_targets[0].parent),
            &native.grant_administrator, native.routes[0].represented_subject.as_ref()).is_err());
        for parent in [&selected.parent, &selected.helper_targets[1].parent, &helper.report_capability.as_ref().unwrap().parent[0]] {
            source.grants().resolve_lineage(std::slice::from_ref(parent), &native.grant_administrator,
                native.routes[0].represented_subject.as_ref()).unwrap().check_current().unwrap();
        }
        drop(custody); drop(machine); drop(originals); drop(source); Ok(())
    }).unwrap();
}

#[test]
fn public_parent_is_one_action_recovers_and_never_relabels_the_account_parent() {
    let mut fixture = Fixture::new();
    // Old installer requests keep exactly their prior serialized form.
    let legacy = serde_json::to_value(&fixture.request).unwrap();
    assert!(legacy.get("public_issuance_id").is_none());
    let roundtrip: HelperReportCapabilityRequest = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(serde_json::to_value(roundtrip).unwrap(), legacy);
    fixture.request.account_issuance_id = Some(EvidenceId::new("separate-account-root").unwrap());
    let account_only = fixture.configure(fixture.helper.clone(), &fixture.request).unwrap();
    fixture.request.public_issuance_id = Some(EvidenceId::new("separate-public-root").unwrap());
    let all = fixture.configure(fixture.helper.clone(), &fixture.request).unwrap();
    assert_eq!(all.parent, account_only.parent);
    assert_eq!(all.account_parent, account_only.account_parent);
    assert_ne!(all.public_parent, all.account_parent);
    let before = fixture.bytes();
    assert!(fixture.configure(fixture.helper.clone(), &fixture.request).unwrap() == all);
    assert_eq!(fixture.bytes(), before);
    // All three ID collision pairs are rejected before even report issuance.
    for id in [&fixture.request.issuance_id, fixture.request.account_issuance_id.as_ref().unwrap()] {
        let mut changed = fixture.request.clone(); changed.public_issuance_id = Some(id.clone());
        assert!(matches!(fixture.configure(fixture.helper.clone(), &changed), Err(HostLifetimeError::InvalidSelection)));
        assert_eq!(fixture.bytes(), before);
    }
    let mut changed = fixture.request.clone(); changed.account_issuance_id = Some(changed.issuance_id.clone());
    assert!(matches!(fixture.configure(fixture.helper.clone(), &changed), Err(HostLifetimeError::InvalidSelection)));
    assert_eq!(fixture.bytes(), before);
    NativeHostLifetime::with_candidate_installation(&fixture.selection, |policy| {
        let native = policy.native_host.as_ref().unwrap(); let operator = native.remote_operator.as_ref().unwrap();
        let (source, originals) = open_owner_pair(&fixture.selection, native, operator, OperatorOwnerStartup::Restore)?;
        let executor = &fixture.helper.logical_executor; let audience = &native.members[0].audience;
        let public = source.grants().resolve_controller_lineage(all.public_parent.as_ref().unwrap(),
            &native.grant_administrator, None).unwrap();
        let account = source.grants().resolve_controller_lineage(all.account_parent.as_ref().unwrap(),
            &native.grant_administrator, None).unwrap();
        assert_eq!(public.restrictions(), &super::super::configured_report::public_parent(&native.grant_administrator, executor, audience));
        assert_eq!(account.restrictions(), &super::super::configured_report::account_parent(&native.grant_administrator, executor, audience));
        assert_eq!(public.restrictions().actions, meerkat_authorization_contracts::constraints::ExactRestriction::exact([
            meerkat_authorization_contracts::constraints::ActionRef {
                feature: "toolkit".into(), action: "setup-public-service-request".into(),
            },
        ]));
        assert_ne!(public.restrictions().actions, account.restrictions().actions);
        public.check_current().unwrap(); account.check_current().unwrap();
        drop(public); drop(account); drop(originals); drop(source); Ok(())
    }).unwrap();
}

#[test]
fn report_parent_recovery_refuses_legacy_unrestricted_id_and_new_id_selects_owned_parent() {
    let fixture = Fixture::new();
    let (legacy, legacy_child, legacy_child_bounds) = NativeHostLifetime::with_candidate_installation(&fixture.selection, |policy| {
        let native = policy.native_host.as_ref().unwrap();
        let operator = native.remote_operator.as_ref().unwrap();
        let (source, originals) = open_owner_pair(&fixture.selection, native, operator, OperatorOwnerStartup::Restore)?;
        let mut bounds = report_parent(&native.grant_administrator, &fixture.helper.logical_executor, &native.members[0].audience);
        bounds.resource_domains = meerkat_authorization_contracts::constraints::DomainRestriction::unrestricted();
        let legacy = source.grants().issue_root(operator, fixture.request.issuance_id.clone(),
            native.grant_administrator.clone(), fixture.helper.represented_subject.clone(), bounds).unwrap();
        let mut child_bounds = crate::installed_host::application_operations::parent_restrictions(
            crate::installed_host::application_operations::setup_report(&native.grant_administrator,
                &fixture.helper.logical_executor, &native.members[0].audience, &meerkat_core::SessionId::new()));
        child_bounds.delegation_depth = DelegationDepth::remaining(0);
        let child = source.grants().issue_child(&native.grant_administrator, &legacy,
            EvidenceId::new("historical-report-child").unwrap(), fixture.helper.logical_executor.clone(), child_bounds.clone()).unwrap();
        drop(originals);
        drop(source);
        Ok((legacy, child, child_bounds))
    }).unwrap();
    let before = fixture.bytes();
    assert!(matches!(fixture.configure(fixture.helper.clone(), &fixture.request), Err(HostLifetimeError::Denied)));
    assert_eq!(fixture.bytes(), before, "old issuance must not be narrowed or relabelled in place");
    let mut fresh = fixture.request.clone();
    fresh.issuance_id = EvidenceId::new("fresh-owned-helper-report-parent").unwrap();
    let selected = fixture.configure(fixture.helper.clone(), &fresh).unwrap();
    let after = fixture.bytes();
    assert_eq!(selected.parent[0].grant_id, fresh.issuance_id);
    assert_ne!(selected.parent[0], legacy);
    assert!(fixture.configure(fixture.helper.clone(), &fresh).unwrap() == selected);
    assert_eq!(fixture.bytes(), after, "the exact fresh issuance is recovered without another append");
    NativeHostLifetime::with_candidate_installation(&fixture.selection, |policy| {
        let native = policy.native_host.as_ref().unwrap();
        let operator = native.remote_operator.as_ref().unwrap();
        let (source, originals) = open_owner_pair(&fixture.selection, native, operator, OperatorOwnerStartup::Restore)?;
        let grants = source.grants();
        assert_eq!(grants.issued_reference(&fixture.request.issuance_id).unwrap(), Some(legacy.clone()));
        let old = grants.resolve_controller_lineage(std::slice::from_ref(&legacy), &native.grant_administrator,
            fixture.helper.represented_subject.as_ref()).unwrap();
        assert_eq!(old.restrictions().resource_domains, meerkat_authorization_contracts::constraints::DomainRestriction::unrestricted());
        let old_child = grants.resolve_controller_lineage(&[legacy.clone(), legacy_child.clone()],
            &fixture.helper.logical_executor, fixture.helper.represented_subject.as_ref()).unwrap();
        assert_eq!(old_child.restrictions(), &legacy_child_bounds);
        let rebound_child = meerkat_authorization::grants::ChildGrantIntent {
            issuer: native.grant_administrator.clone(), parent_lineage: selected.parent.clone(),
            id: legacy_child.grant_id.clone(), grantee: fixture.helper.logical_executor.clone(),
            represented_subject: fixture.helper.represented_subject.clone(), requested: legacy_child_bounds.clone(),
        };
        assert_eq!(grants.issue_or_recover_child_under_exclusion(&rebound_child, || Ok(meerkat_authorization::grants::ChildGrantValidity {
            not_before_ms: 0, expires_at_ms: u64::MAX,
            deadline: meerkat_core::time_compat::Instant::now() + std::time::Duration::from_secs(60),
        })), Err(meerkat_authorization::grants::ChildGrantRefusal::Denied));
        assert_eq!(fixture.bytes(), after, "a historical child cannot be recovered under another parent");
        let current = grants.resolve_controller_lineage(&selected.parent, &native.grant_administrator,
            fixture.helper.represented_subject.as_ref()).unwrap();
        assert_eq!(current.restrictions(), &report_parent(&native.grant_administrator,
            &fixture.helper.logical_executor, &native.members[0].audience));
        drop(current);
        drop(old_child);
        drop(old);
        drop(grants);
        drop(originals);
        drop(source);
        Ok(())
    }).unwrap();
}
