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
    let (composer, reviews) =
        SelectedReviewComposer::new(selected.clone(), facade, realm.clone(), approvals.clone())
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
    Arc::new(McpRouterAdapter::new(
        meerkat::McpRouter::new_with_stdio_profile(meerkat::McpStdioLaunchProfile::trusted_host()),
    ))
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
        assert!(matches!(
            require_gateway_member_realm(&fixture.selected, &wrong),
            Err(HostLifetimeError::WrongOwner)
        ));
    }
    let foreign = MobMemberBinding {
        mob_id: "other-application".into(),
        ..fixture.selected.clone()
    };
    assert!(matches!(
        require_gateway_member_realm(&foreign, &fixture.realm),
        Err(HostLifetimeError::WrongOwner)
    ));
    assert!(matches!(
        fixture.reviews.try_recv(),
        Err(TryRecvError::Empty)
    ));
    fixture
        .composer
        .compose(&adapter(), &session, Some(&fixture.selected))
        .unwrap();
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
    assert!(matches!(
        fixture.reviews.try_recv(),
        Err(TryRecvError::Empty)
    ));
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
    assert!(matches!(
        fixture.reviews.try_recv(),
        Err(TryRecvError::Empty)
    ));
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
    {
        let member = MobMemberBinding {
            role: "private-role-sentinel".into(),
            ..fixture.selected.clone()
        };
        let error = fixture
            .composer
            .compose(&adapter, &SessionId::new(), Some(&member))
            .err()
            .expect("wrong role must refuse");
        assert_safe_refusal(&error, "member_selection", "role_mismatch", &[&member.role]);
    }
    assert!(matches!(
        fixture.reviews.try_recv(),
        Err(TryRecvError::Empty)
    ));
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
        let error = fixture
            .composer
            .compose(&adapter(), &rebuilt_session, Some(&fixture.selected))
            .err()
            .expect("existing session owner must not rebind");
        assert_safe_refusal(
            &error,
            "session_binding",
            "invalid_input",
            &[&rebuilt_session.to_string()],
        );
        assert!(fixture.approvals.check_session(&session).is_ok());
        assert!(matches!(
            fixture.reviews.try_recv(),
            Err(TryRecvError::Empty)
        ));
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
    let error = fixture
        .composer
        .compose(&adapter(), &session, Some(&fixture.selected))
        .err()
        .expect("closed receiver must refuse");
    assert_safe_refusal(
        &error,
        "review_transfer",
        "receiver_closed",
        &[&session.to_string()],
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
        assert!(matches!(
            fixture.reviews.try_recv(),
            Err(TryRecvError::Empty)
        ));
    }
}

fn assert_safe_refusal(
    error: &McpAdapterComposeRefusal,
    stage: &str,
    cause: &str,
    private: &[&str],
) {
    let expected =
        format!("The selected setup review could not be composed (stage: {stage}; cause: {cause})");
    assert_eq!(error.reason, expected);
    assert_eq!(error.to_string(), expected);
    for value in private {
        assert!(
            !error.reason.contains(value),
            "private value appeared in the refusal"
        );
        assert!(
            !format!("{error:?}").contains(value),
            "private value appeared in Debug"
        );
    }
}

#[test]
fn typed_diagnostics_never_render_sensitive_error_fields_or_nested_sources() {
    const PRIVATE: &str = "private-token-account-path-sentinel";
    let native = || ConnectError::native(PRIVATE, std::io::Error::other(PRIVATE));
    let cases = [
        (ConnectError::ReviewCancelled, "review_cancelled"),
        (
            ConnectError::provider_setup(PRIVATE, std::io::Error::other(PRIVATE)),
            "provider_setup",
        ),
        (native(), "native"),
        (ConnectError::InvalidInput(PRIVATE.into()), "invalid_input"),
        (ConnectError::MissingRegistration, "missing_registration"),
        (ConnectError::MissingAttachment, "missing_attachment"),
        (
            ConnectError::Cleanup {
                primary: Box::new(native()),
                cleanup: Box::new(native()),
            },
            "cleanup",
        ),
    ];
    for (error, cause) in cases {
        assert_safe_refusal(
            &connect_composition_refused("review_composition", &error),
            "review_composition",
            cause,
            &[PRIVATE],
        );
    }
}

#[cfg(all(unix, feature = "console"))]
fn terminal_fixture(
    fixture: &Fixture,
    presenters: crate::application_gateway::LocalSetupApprovals,
) -> TerminalReviewComposer {
    TerminalReviewComposer {
        selected: fixture.selected.clone(),
        realm: fixture.realm.clone(),
        auth: fixture.composer.auth.clone(),
        presenters,
        tasks: None,
        realm_installation: None,
    }
}

#[cfg(all(unix, feature = "console"))]
#[tokio::test]
async fn actual_presenter_creation_refusal_withholds_the_private_storage_path() {
    let fixture = fixture();
    let private_parent = fixture
        ._directory
        .path()
        .join("private-storage-path-sentinel");
    // A missing parent is refused by the real LocalApprovals creation path.
    let (presenters, _reviews) =
        crate::application_gateway::setup_presenter::channel(private_parent.clone());
    let composer = terminal_fixture(&fixture, presenters);
    let error = composer
        .compose(&adapter(), &SessionId::new(), Some(&fixture.selected))
        .err()
        .expect("missing private storage parent must refuse");
    assert_safe_refusal(
        &error,
        "presenter_enrollment",
        "invalid_input",
        &[
            "private-storage-path-sentinel",
            private_parent.to_str().unwrap(),
        ],
    );
}

#[cfg(all(unix, feature = "console"))]
#[tokio::test]
async fn actual_presenter_transfer_failure_is_not_misattributed_to_the_completed_callback() {
    let fixture = fixture();
    let (presenters, reviews) =
        crate::application_gateway::setup_presenter::channel(fixture._directory.path().into());
    let composer = terminal_fixture(&fixture, presenters);
    let adapter = adapter();
    let first = composer
        .compose(&adapter, &SessionId::new(), Some(&fixture.selected))
        .unwrap();
    // The actual bounded enrollment channel is full. A second fresh review is
    // composed, then transfer refuses. Do not report review_composition here.
    let second_session = SessionId::new();
    let error = composer
        .compose(&adapter, &second_session, Some(&fixture.selected))
        .err()
        .expect("full presenter channel must refuse");
    assert_safe_refusal(
        &error,
        "presenter_enrollment",
        "invalid_input",
        &[&second_session.to_string()],
    );
    drop(reviews);
    let error = composer
        .compose(&adapter, &SessionId::new(), Some(&fixture.selected))
        .err()
        .expect("closed presenter channel must refuse before callback");
    assert_safe_refusal(&error, "presenter_enrollment", "invalid_input", &[]);
    drop(first);
}
