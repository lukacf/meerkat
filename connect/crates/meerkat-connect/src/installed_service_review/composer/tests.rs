use super::*;
use meerkat_auth_core::auth_store::{
    EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence,
};
use meerkat_core::RealmId;
use std::sync::mpsc::TryRecvError;

struct Fixture {
    _directory: tempfile::TempDir,
    _runtime: meerkat_runtime::MeerkatMachine,
    _presenter: crate::approval::ApprovalPresenter,
    composer: Arc<SelectedReviewComposer>,
    reviews: Receiver<InstalledServiceReview>,
    approvals: Arc<LocalApprovals>,
    selected: MobMemberBinding,
    realm: RealmLocator,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let persistence = ProviderAuthPersistence::new(
        Arc::new(EphemeralTokenStore::new()),
        Arc::new(InMemoryCoordinator::new()),
    );
    // Match the gateway offer: its shared-store MCP realm differs from the
    // member realm, while the connector owner keeps the same native runtime.
    let facade = HostAuthService::new(persistence, runtime.provider_auth_runtime_authority())
        .with_mcp_realm(RealmId::parse("meta").unwrap());
    let realm = RealmLocator {
        state_root: directory.path().join("realm-root"),
        realm: RealmId::parse("mob.application").unwrap(),
    };
    let selected = MobMemberBinding {
        mob_id: "application".into(),
        role: "selected-setup".into(),
        member: "helper-one".into(),
    };
    let (approvals, presenter) = LocalApprovals::create(directory.path()).unwrap();
    let approvals = Arc::new(approvals);
    let (composer, reviews) = SelectedReviewComposer::new(
        selected.clone(),
        facade,
        realm.clone(),
        approvals.clone(),
    )
    .unwrap();
    Fixture {
        _directory: directory,
        _runtime: runtime,
        _presenter: presenter,
        composer,
        reviews,
        approvals,
        selected,
        realm,
    }
}

fn adapter() -> Arc<McpRouterAdapter> {
    // Empty native router only. These controls start no MCP server, model,
    // provider request or native actor, and do not establish native admission.
    Arc::new(McpRouterAdapter::new(meerkat::McpRouter::new()))
}

#[cfg(all(unix, feature = "console"))]
#[tokio::test]
async fn gateway_member_realm_refuses_a_foreign_selection_before_review_binding() {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    let fixture = fixture();
    let session = SessionId::new();
    require_gateway_member_realm(&fixture.selected, &fixture.realm).unwrap();
    for name in ["meta", "mob.other-application"] {
        let mut wrong = fixture.realm.clone();
        wrong.realm = RealmId::parse(name).unwrap();
        assert!(matches!(require_gateway_member_realm(&fixture.selected, &wrong),
            Err(HostLifetimeError::WrongOwner)));
    }
    let foreign = MobMemberBinding { mob_id: "other-application".into(), ..fixture.selected.clone() };
    assert!(matches!(require_gateway_member_realm(&foreign, &fixture.realm),
        Err(HostLifetimeError::WrongOwner)));
    assert!(matches!(fixture.reviews.try_recv(), Err(TryRecvError::Empty)));
    fixture.composer.compose(&adapter(), &session, Some(&fixture.selected)).unwrap();
    let review = fixture.reviews.try_recv().unwrap();
    assert_eq!(review.realm, fixture.realm);
    assert_eq!(review.auth.realm, fixture.realm.realm);
    assert!(Arc::ptr_eq(&review.auth, &fixture.composer.auth));
    assert!(Arc::ptr_eq(&review.approvals, &fixture.approvals));
    assert!(fixture.approvals.check_session(&session).is_ok());
    drop(review);
    assert!(fixture.approvals.check_session(&session).is_err());
}

#[tokio::test]
async fn exact_native_construction_transfers_the_same_checked_review() {
    let fixture = fixture();
    let adapter = adapter();
    let session = SessionId::new();
    let dispatcher = fixture
        .composer
        .compose(&adapter, &session, Some(&fixture.selected))
        .unwrap();
    let review = fixture.reviews.try_recv().unwrap();
    assert_eq!(review.session, session);
    assert_eq!(review.realm, fixture.realm);
    assert!(Arc::ptr_eq(&review.approvals, &fixture.approvals));
    assert!(Arc::ptr_eq(&review.auth, &fixture.composer.auth));
    assert!(Arc::ptr_eq(
        &review.call_context_provider(),
        &fixture.composer.auth.context_provider
    ));
    assert!(Arc::ptr_eq(&dispatcher, &review.checked_dispatcher()));
    let raw: Arc<dyn AgentToolDispatcher> = adapter;
    assert!(!Arc::ptr_eq(&dispatcher, &raw));
    assert!(fixture.approvals.check_session(&session).is_ok());
    assert!(matches!(fixture.reviews.try_recv(), Err(TryRecvError::Empty)));
    drop(review);
    assert!(fixture.approvals.check_session(&session).is_err());
}

#[tokio::test]
async fn unrelated_and_non_member_constructions_keep_the_exact_adapter() {
    let fixture = fixture();
    let adapter = adapter();
    let raw: Arc<dyn AgentToolDispatcher> = adapter.clone();
    let unrelated = MobMemberBinding {
        member: "researcher".into(),
        role: "research".into(),
        ..fixture.selected.clone()
    };
    let same_role = MobMemberBinding {
        member: "second-setup-helper".into(),
        ..fixture.selected.clone()
    };
    let other_mob = MobMemberBinding {
        mob_id: "other-application".into(),
        ..fixture.selected.clone()
    };
    for member in [None, Some(&unrelated), Some(&same_role), Some(&other_mob)] {
        let dispatcher = fixture
            .composer
            .compose(&adapter, &SessionId::new(), member)
            .unwrap();
        assert!(Arc::ptr_eq(&dispatcher, &raw));
    }
    assert!(matches!(fixture.reviews.try_recv(), Err(TryRecvError::Empty)));
    // Pass-through did not consume the selected private owner.
    assert!(
        fixture
            .composer
            .compose(&adapter, &SessionId::new(), Some(&fixture.selected))
            .is_ok()
    );
}

#[tokio::test]
async fn selected_member_under_a_different_role_refuses_without_binding() {
    let fixture = fixture();
    let adapter = adapter();
    for member in [MobMemberBinding {
        role: "different-role".into(),
        ..fixture.selected.clone()
    }] {
        assert!(
            fixture
                .composer
                .compose(&adapter, &SessionId::new(), Some(&member))
                .is_err()
        );
    }
    assert!(matches!(fixture.reviews.try_recv(), Err(TryRecvError::Empty)));
    assert!(
        fixture
            .composer
            .compose(&adapter, &SessionId::new(), Some(&fixture.selected))
            .is_ok()
    );
}

#[tokio::test]
async fn a_rebuild_cannot_rebind_or_replace_the_existing_review() {
    let fixture = fixture();
    let session = SessionId::new();
    fixture
        .composer
        .compose(&adapter(), &session, Some(&fixture.selected))
        .unwrap();
    let review = fixture.reviews.try_recv().unwrap();
    for rebuilt_session in [session.clone(), SessionId::new()] {
        assert!(
            fixture
                .composer
                .compose(&adapter(), &rebuilt_session, Some(&fixture.selected))
                .is_err()
        );
        assert!(fixture.approvals.check_session(&session).is_ok());
        assert!(matches!(fixture.reviews.try_recv(), Err(TryRecvError::Empty)));
    }
    drop(review);
    assert!(
        fixture
            .composer
            .compose(&adapter(), &session, Some(&fixture.selected))
            .is_err()
    );
}

#[tokio::test]
async fn a_closed_private_receiver_refuses_and_retires_the_review() {
    let fixture = fixture();
    let session = SessionId::new();
    drop(fixture.reviews);
    assert!(
        fixture
            .composer
            .compose(&adapter(), &session, Some(&fixture.selected))
            .is_err()
    );
    assert!(fixture.approvals.check_session(&session).is_err());
    assert!(fixture.approvals.bind_session(&SessionId::new()).is_err());
    // Adapter shutdown is the native failed-construction owner's job. This
    // direct callback test does not exercise that native cleanup path.
}

#[tokio::test]
async fn dropping_an_unclaimed_review_retires_its_private_owner() {
    let fixture = fixture();
    let session = SessionId::new();
    fixture
        .composer
        .compose(&adapter(), &session, Some(&fixture.selected))
        .unwrap();
    assert!(fixture.approvals.check_session(&session).is_ok());
    drop(fixture.reviews);
    assert!(fixture.approvals.check_session(&session).is_err());
}

#[tokio::test]
async fn prebound_or_retired_private_owners_are_not_rearmed() {
    for retired in [false, true] {
        let fixture = fixture();
        let session = SessionId::new();
        if retired {
            fixture.approvals.retire();
        } else {
            fixture.approvals.bind_session(&session).unwrap();
        }
        #[cfg(all(unix, feature = "console"))]
        require_gateway_member_realm(&fixture.selected, &fixture.realm).unwrap();
        assert!(
            fixture
                .composer
                .compose(&adapter(), &session, Some(&fixture.selected))
                .is_err()
        );
        assert!(matches!(fixture.reviews.try_recv(), Err(TryRecvError::Empty)));
    }
}
