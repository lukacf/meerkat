//! Actual installed policy, registered bundle witnesses and retained source
//! rows. Native dispatch, input admission and human presentation are not run.
use super::*;
use crate::original_ingress::helper_originals::{
    HelperAdmissionSelection, HelperNativeBinding, HelperSetupOrigin, retained_policy_fixture,
};
use meerkat_core::service::WorkGraphNamespaceGrant;
use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphService};

struct HelperFixture {
    installed: Fixture,
    association: InputAuthorityAssociation,
    origin: HelperSetupOrigin,
    session: SessionId,
}
impl HelperFixture {
    async fn new() -> Self {
        Self::from_installed(Fixture::new().await, SessionId::new(), Some(u64::MAX)).await
    }

    async fn with_deadline(deadline_ms: Option<u64>, grant_expiry: Option<u64>) -> Self {
        let session = SessionId::new();
        let helper_session = session.clone();
        let installed = Fixture::with_operations(move |native, _| {
            let mut bounds = memory_bounds(native, &helper_session);
            if let Some(expiry) = grant_expiry {
                bounds.lifetime = meerkat_authorization_contracts::constraints::LifetimeRestriction::window(0, expiry);
            }
            bounds
        }).await;
        Self::from_installed(installed, session, deadline_ms).await
    }

    async fn from_installed(installed: Fixture, session: SessionId, deadline_ms: Option<u64>) -> Self {
        let native = &installed.application.native;
        assert!(installed.association.candidate().original_work.authority != native.ingress_domain.authority);
        let controller_parent = native.routes[0].controller_parent.clone().unwrap();
        let operation_parent = native.routes[0].application_parent.clone().unwrap();
        let executor = native.routes[0].logical_executor.clone();
        let child = |parent: &[GrantLineageRef], label: &str| {
            let leaf = installed.source.grants().issue_child(
                &native.grant_administrator, parent.last().unwrap(), id(label), executor.clone(),
                ExecutionRestrictions { delegation_depth: DelegationDepth::remaining(0),
                    ..ExecutionRestrictions::unrestricted() },
            ).unwrap();
            let mut lineage = parent.to_vec(); lineage.push(leaf); lineage
        };
        // Independent real children preserve the existing fixture's memory
        // ceiling. The local operation projection below is not a report grant.
        let controller = child(&controller_parent, "origin-policy-controller");
        let basis = WorkAuthorityBasis::GrantLineage {
            lineage: child(&operation_parent, "origin-policy-memory"),
        };
        let member = native.members[0].clone();
        let selection = HelperAdmissionSelection {
            helper: super::super::acquisition::InstalledSetupSelection {
                catalog_capability: None,
                report_capability: None, member: member.member.clone(),
                ingress_actor: super::super::tests::principal("helper-owner"),
                logical_executor: executor, represented_subject: None,
                controller_parent, operation_basis: basis.clone(),
            },
            member, grant_caller: native.grant_administrator.clone(),
        };
        assert_ne!(session, installed.session);
        let mut target = installed.association.candidate().target.clone();
        target.context = id(&session.to_string());
        target.logical_runtime = id(&meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string());
        let binding = HelperNativeBinding {
            ingress_actor: selection.helper.ingress_actor.clone(),
            logical_executor: selection.helper.logical_executor.clone(), represented_subject: None,
            target, authority_basis: basis, controller_grant_lineage: controller,
            controller_model: selected(&installed.facts),
            controller_ceiling: installed.association.candidate().controller_ceiling.clone(),
            admitted_ceiling: installed.association.candidate().admitted_ceiling.clone(),
        };
        let service = WorkGraphService::with_namespace_grant(
            Arc::new(SqliteWorkGraphStore::open(installed._directory.path().join("origin-policy.sqlite3")).unwrap()),
            WorkGraphNamespaceGrant::new(native.realm.to_string(), "setup").unwrap(),
        ).unwrap();
        let source = retained_policy_fixture(
            &installed.source, installed.application.originals.canonical_store_for_policy_fixture(),
            StoreScope { realm: native.realm.to_string(), owner: installed.application.selected.deployment.to_string() },
            &service, "origin-policy", installed.retained.clone(), vec![installed.retained.clone()],
            selection, binding,
        ).await;
        let origin = source.policy_source().unwrap().origin;
        assert!(origin.original_authority == installed.association.candidate().original_work.authority);
        assert!(origin.source_owner == installed.association.candidate().ingress_namespace.ingress);
        let association = source.original.association().clone();
        installed.application.originals.retain_helper_policy_deadline_fixture(source, deadline_ms);
        Self { installed, association, origin, session }
    }

    fn binding(&self, call: &SetupBundleCall) -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000005\"").unwrap(),
            execution_scope: scope(&self.session), run_id: None, context_revision: None,
            operation: AuthorizationOperation::Tool(call.facts.clone()),
        })
    }

    fn values(&self, call: &SetupBundleCall)
        -> Result<LocalPolicyAllowance, OperationAuthorizationError>
    {
        self.installed.application.authorize_operation(
            &self.association, &self.binding(call), LocalPolicyPurpose::Operation, 0,
        )
    }
}

fn calls() -> [SetupBundleCall; 3] {
    [
        SetupBundleCall::new(meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE, meerkat_toolkit::setup::SETUP_REPORT),
        SetupBundleCall::new(meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE, meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST),
        SetupBundleCall::new(meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE, meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST),
    ]
}

#[tokio::test]
async fn registered_helper_operations_project_the_retained_qualified_origin() {
    let fixture = HelperFixture::new().await;
    let native = &fixture.installed.application.native;
    let claims = fixture.association.candidate();
    let [report, account, public] = calls();
    let report_values = fixture.values(&report).unwrap().operation_values.remove(0);
    let account_values = fixture.values(&account).unwrap().operation_values.remove(0);
    let public_values = fixture.values(&public).unwrap().operation_values.remove(0);
    let expected = application_operations::setup_report_for_origin(
        &native.grant_administrator, &claims.logical_executor, &claims.target.audience, &fixture.origin,
    ).unwrap();
    assert_eq!(report_values.resource_domain, expected.resource_domain);
    assert_eq!(report_values.action, expected.action);
    assert_eq!(report_values.processor, expected.processor);
    assert_eq!(report_values.audience, expected.audience);
    assert_eq!(report_values.resource_domain.authority, native.grant_administrator);
    assert_ne!(report_values.resource_domain, account_values.resource_domain);
    assert_ne!(report_values.action, account_values.action);
    assert_ne!(public_values.action, account_values.action);
    assert_ne!(public_values.action, report_values.action);
    assert_ne!(public_values.resource_domain, account_values.resource_domain);
    assert_ne!(public_values.resource_domain, report_values.resource_domain);
    let public_expected = application_operations::setup_public_service_request_for_origin(
        &native.grant_administrator, &claims.logical_executor, &claims.target.audience, &fixture.origin,
    ).unwrap();
    assert_eq!(public_values.action, public_expected.action);
    assert_eq!(public_values.resource_domain, public_expected.resource_domain);
    assert_eq!(public_values.processor, public_expected.processor);
    assert_eq!(public_values.audience, public_expected.audience);
    assert_eq!(account_values.resource_domain, application_operations::setup_account_request_for_origin(
        &native.grant_administrator, &claims.logical_executor, &claims.target.audience, &fixture.origin,
    ).unwrap().resource_domain);
    // These rows have only the fixture's memory grant. The real native
    // restriction owner still excludes all three projected operations.
    let lineage = claims.authority_basis.lineage_for(None).unwrap();
    let grant = fixture.installed.source.grants().resolve_lineage(
        lineage, &claims.logical_executor, claims.represented_subject.as_ref(),
    ).unwrap();
    assert!(!in_bounds(grant.restrictions(), &report_values));
    assert!(!in_bounds(grant.restrictions(), &account_values));
    assert!(!in_bounds(grant.restrictions(), &public_values));
    assert_eq!(report.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(account.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(public.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn helper_projection_refuses_nonhelper_changed_target_and_closed_owner() {
    let fixture = HelperFixture::new().await;
    for call in calls() {
        denied(fixture.installed.application.authorize_operation(
            &fixture.installed.association, &fixture.binding(&call), LocalPolicyPurpose::Operation, 0,
        ));
        for field in 0..4 {
            let mut changed = fixture.association.candidate().clone();
            match field {
                0 => changed.target.context = id(&SessionId::new().to_string()),
                1 => changed.logical_executor = super::super::tests::principal("other-helper"),
                2 => changed.target.audience = AudienceRef::Principal {
                    principal: super::super::tests::principal("other-audience"),
                },
                3 => changed.ingress_namespace.realm = meerkat_core::RealmId::parse("other-realm").unwrap(),
                _ => unreachable!(),
            }
            let changed = InputAuthorityAssociation::new(changed).unwrap();
            assert!(fixture.installed.application.authorize_operation(
                &changed, &fixture.binding(&call), LocalPolicyPurpose::Operation, 0,
            ).is_err());
        }
        assert_eq!(call.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    fixture.installed.lifetime.begin_close_activation(HostCloseReason::OperatorDisable).unwrap();
    for call in calls() {
        denied(fixture.values(&call));
        assert_eq!(call.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}

fn in_bounds(bounds: &ExecutionRestrictions, values: &meerkat_authorization::policy::LocalOperationValues) -> bool {
    bounds.check_bounds(meerkat_authorization_contracts::constraints::OperationRestrictionValues {
        action: &values.action, resource_domain: &values.resource_domain,
        processor: &values.processor, audience: &values.audience, now_ms: 0,
    }).is_ok()
}


// This clock controls actual native prepared authorization, not a timer sleep.
// The durable grant owner and retained source policy remain the real fixtures.
struct ReviewClock {
    start: meerkat_authorization::clock::LocalAuthorizationTime,
    elapsed: std::sync::atomic::AtomicU64,
}
impl ReviewClock {
    fn new() -> Arc<Self> {
        use meerkat_authorization::clock::LocalAuthorizationClock;
        Arc::new(Self {
            start: HostAuthorizationClock.now().unwrap(),
            elapsed: std::sync::atomic::AtomicU64::new(0),
        })
    }
    fn at(&self, elapsed: u64) { self.elapsed.store(elapsed, std::sync::atomic::Ordering::SeqCst); }
    fn deadline(&self, elapsed: u64) -> u64 { self.start.unix_ms + elapsed }
}
impl meerkat_authorization::clock::LocalAuthorizationClock for ReviewClock {
    fn now(&self) -> Result<meerkat_authorization::clock::LocalAuthorizationTime,
        meerkat_authorization::clock::LocalClockError>
    {
        let elapsed = self.elapsed.load(std::sync::atomic::Ordering::SeqCst);
        Ok(meerkat_authorization::clock::LocalAuthorizationTime {
            unix_ms: self.deadline(elapsed),
            monotonic: self.start.monotonic + std::time::Duration::from_millis(elapsed),
        })
    }
}
impl HelperFixture {
    fn native_context(&self, clock: Arc<ReviewClock>) -> WorkAuthorizationContext {
        WorkAuthorizationContext::new(Arc::new(meerkat_authorization::work::LocalWorkAuthorization::new(
            Arc::new(self.association.clone()), self.installed.policy.clone(),
            self.installed.source.publication(), clock,
        )), scope(&self.session))
    }
    fn memory_binding(&self) -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000006\"").unwrap(),
            execution_scope: scope(&self.session), run_id: None, context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                usage: SourceAuthorizationUse::Read,
                target: SourceAuthorizationTarget::Memory(MemorySearchScope::for_session(self.session.clone())),
            }),
        })
    }
}

#[tokio::test]
async fn helper_review_lifetime_keeps_same_native_preparation_current_past_sixty_seconds() {
    let clock = ReviewClock::new();
    let fixture = HelperFixture::with_deadline(Some(clock.deadline(180_000)), None).await;
    let context = fixture.native_context(clock.clone());
    let binding = fixture.memory_binding();
    let prepared = context.authorization().prepare(&binding).unwrap();
    clock.at(108_475); // The actual failed review wait, without sleeping or re-preparing.
    prepared.check_current(&binding).unwrap();
    let now = clock.deadline(108_475);
    for purpose in [LocalPolicyPurpose::Operation, LocalPolicyPurpose::Controller] {
        let expected = if purpose == LocalPolicyPurpose::Operation { clock.deadline(180_000) } else { now + 60_000 };
        assert_eq!(fixture.installed.application.authorize_admitted_work(
            &fixture.association, &binding, purpose, now,
        ).unwrap().expires_at_ms, expected);
    }
    assert_eq!(fixture.installed.application.authorize_operation(
        &fixture.association, &binding, LocalPolicyPurpose::Operation, now,
    ).unwrap().expires_at_ms, clock.deadline(180_000));
    clock.at(180_000);
    denied(prepared.check_current(&binding));
    denied(context.authorization().prepare(&binding));
}

#[tokio::test]
async fn helper_review_lifetime_retains_shorter_setup_and_native_grant_expiry() {
    for (setup_ms, grant_ms) in [(45_000, 180_000), (180_000, 90_000)] {
        let clock = ReviewClock::new();
        let fixture = HelperFixture::with_deadline(Some(clock.deadline(setup_ms)),
            Some(clock.deadline(grant_ms))).await;
        let context = fixture.native_context(clock.clone());
        let binding = fixture.memory_binding();
        let prepared = context.authorization().prepare(&binding).unwrap();
        clock.at(setup_ms.min(grant_ms) - 1);
        prepared.check_current(&binding).unwrap();
        clock.at(setup_ms.min(grant_ms));
        denied(prepared.check_current(&binding));
        denied(context.authorization().prepare(&binding));
    }
}

#[tokio::test]
async fn helper_review_lifetime_missing_live_deadline_never_falls_back_to_ordinary_work() {
    let clock = ReviewClock::new();
    let fixture = HelperFixture::with_deadline(None, None).await;
    let binding = fixture.memory_binding();
    assert!(matches!(fixture.native_context(clock.clone()).authorization().prepare(&binding),
        Err(OperationAuthorizationError::Unavailable)));
    assert!(matches!(fixture.installed.application.authorize_admitted_work(
        &fixture.association, &binding, LocalPolicyPurpose::Operation, clock.start.unix_ms,
    ), Err(OperationAuthorizationError::Unavailable)));
    // Controller custody remains separately native-owned, including during close.
    assert_eq!(fixture.installed.application.authorize_admitted_work(
        &fixture.association, &binding, LocalPolicyPurpose::Controller, clock.start.unix_ms,
    ).unwrap().expires_at_ms, clock.deadline(60_000));
    assert_eq!(fixture.installed.application.allowance_deadline(
        &fixture.installed.association, LocalPolicyPurpose::Operation, clock.start.unix_ms,
    ).unwrap(), clock.deadline(60_000));
    let mut foreign = fixture.association.candidate().clone();
    foreign.target.context = id(&SessionId::new().to_string());
    let foreign = InputAuthorityAssociation::new(foreign).unwrap();
    assert_eq!(fixture.installed.application.originals.helper_operation_deadline(&foreign).unwrap(), None);
    assert!(fixture.installed.application.authorize_admitted_work(
        &foreign, &binding, LocalPolicyPurpose::Operation, clock.start.unix_ms,
    ).is_err());
}

#[tokio::test]
async fn helper_review_lifetime_publication_change_still_refuses_entry_after_wait() {
    for change in ["revoke", "close", "cancel"] {
        let clock = ReviewClock::new();
        let fixture = HelperFixture::with_deadline(Some(clock.deadline(180_000)), None).await;
        let context = fixture.native_context(clock.clone());
        let binding = fixture.memory_binding();
        let prepared = context.authorization().prepare(&binding).unwrap();
        clock.at(108_475);
        prepared.check_current(&binding).unwrap();
        match change {
            "revoke" => {
                let machine = meerkat_runtime::MeerkatMachine::ephemeral().with_local_grant_authorization(
                    fixture.installed.source.configuration(
                        ScopedInvocation::from_policy(fixture.installed.application.clone()),
                        fixture.installed.application.clone(), None),
                ).unwrap();
                let mut custody = machine.try_controller_grant_mutation().unwrap();
                fixture.installed.source.grants().revoke(
                    &fixture.installed.application.native.grant_administrator,
                    fixture.association.candidate().authority_basis.lineage_for(None).unwrap().last().unwrap(),
                    &mut custody,
                ).unwrap();
            }
            "close" => { fixture.installed.lifetime.begin_close_activation(HostCloseReason::OperatorDisable).unwrap(); }
            "cancel" => {
                // Cancel the exact canonical parent retained by its real owner.
                // The helper policy fixture's synthetic origin locator is not
                // the source-returned event reference used for cancellation.
                let originals = &fixture.installed.application.originals;
                assert_eq!(originals.binding_evidence(&fixture.installed.reference).await.unwrap(),
                    Some(fixture.installed.association.clone()));
                assert_eq!(fixture.installed.reference.original,
                    fixture.installed.retained.association().candidate().original_work.work);
                originals.record_operation_cancelled(&fixture.installed.reference).await.unwrap();
            }
            _ => unreachable!(),
        }
        assert!(prepared.check_current(&binding).is_err(), "{change}");
        // Native current_tool reparses only ReprepareRequired. A new preparation
        // must still refuse; observing a publication change is not permission.
        let entries = std::sync::atomic::AtomicUsize::new(0);
        let current = context.authorization().prepare(&binding);
        if let Ok(current) = &current {
            if current.check_current(&binding).is_ok() {
                entries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        assert!(current.is_err(), "{change}");
        assert_eq!(entries.load(std::sync::atomic::Ordering::SeqCst), 0, "{change}");
    }
}
