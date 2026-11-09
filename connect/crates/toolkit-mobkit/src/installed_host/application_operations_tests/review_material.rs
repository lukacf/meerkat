//! Current owner material through actual native grants and retained sources.
//! Component scope only: no native input execution or protected panel is claimed.
use super::*;
use meerkat_core::authorization::{ReviewOperationContext, ReviewOperationContextResolution};
use std::sync::atomic::{AtomicUsize, Ordering};

struct ReviewFloor(AtomicUsize);
impl InstalledOperationReviewPolicy for ReviewFloor {
    fn review_floor(
        &self,
        _: InstalledOperationReviewContext<'_>,
    ) -> Result<OperationReviewTier, OperationAuthorizationError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(OperationReviewTier::R3)
    }
}
fn material(
    result: Result<ReviewOperationContextResolution, OperationAuthorizationError>,
) -> ReviewOperationContext {
    match result.unwrap() {
        ReviewOperationContextResolution::Material(value) => value,
        ReviewOperationContextResolution::NotApplicable => {
            panic!("the installed work has a current mandate")
        }
    }
}
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .try_into()
        .unwrap()
}
fn call(account: meerkat_core::OperationAccount) -> SetupBundleCall {
    SetupBundleCall::with_account(
        crate::FIND_AND_INSTALL_BUNDLE,
        crate::FIND_AND_INSTALL,
        Some(account),
    )
}

#[tokio::test]
async fn installed_review_material_uses_live_grant_bounds_and_owner_account() {
    let floor = Arc::new(ReviewFloor(AtomicUsize::new(0)));
    let f =
        Fixture::with_review_policy(setup_request_bounds, true, false, true, Some(floor.clone()))
            .await;
    for (account, expected) in [
        (meerkat_core::OperationAccount::DeclaredNone, None),
        (
            meerkat_core::OperationAccount::selected("https://owner.invalid", "selected-user"),
            Some("selected-user at https://owner.invalid"),
        ),
    ] {
        let c = call(account);
        let b = c.binding(&f);
        assert_eq!(
            f.context.authorization().prepare(&b).unwrap().review_tier(),
            OperationReviewTier::R3
        );
        let before = floor.0.load(Ordering::SeqCst);
        let (_, stamp) = f.source.publication().observe(|| ()).unwrap();
        let m = material(f.context.authorization().review_operation_context(&b));
        assert_eq!(m.action_account(), expected);
        let mandate = m.mandate().unwrap();
        assert!(mandate.contains("current local-console source policy"));
        // Resolve both real native parents independently of the new helper.
        // The populated base permits memory, whereas this target permits Find.
        let claims = f.association.candidate();
        let WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(base),
            targets,
        } = &claims.authority_basis
        else {
            panic!("distinct base and selected target required")
        };
        assert_eq!(targets.len(), 1);
        let render = |lineage: &[GrantLineageRef]| {
            let grant = f
                .source
                .grants()
                .resolve_lineage(
                    lineage,
                    &claims.logical_executor,
                    claims.represented_subject.as_ref(),
                )
                .unwrap();
            meerkat_runtime::meerkat_machine::review_bounds_text(
                &grant.restrictions().conjoin(&claims.admitted_ceiling),
                grant.expires_at_ms(),
                now_ms(),
            )
            .unwrap()
        };
        let selected_bounds = render(&targets[0].lineage);
        let base_bounds = render(base);
        assert_ne!(selected_bounds, base_bounds);
        assert!(mandate.contains(&selected_bounds));
        assert!(
            !mandate.contains(&base_bounds),
            "targeted work must not describe its fallback base"
        );
        assert!(mandate.contains("not human consent"));
        assert!(!mandate.contains("fixture input"));
        assert_eq!(
            floor.0.load(Ordering::SeqCst),
            before,
            "material must not rerun review-floor policy"
        );
        stamp.check_current().unwrap();
        assert_eq!(c.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn prepared_review_material_requires_exact_adoption_and_current_source() {
    let f = Fixture::with_targeted_setup().await;
    let prepared = Arc::new(PreparedApplicationPolicy::new(
        f.application.originals.clone(),
    ));
    let configuration = f.source.configuration(
        ScopedInvocation::from_policy(prepared.clone()),
        prepared.clone(),
        None,
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        configuration.grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    let context = policy
        .work_context(vec![f.association.clone()].into(), scope(&f.session))
        .unwrap();
    let c = call(meerkat_core::OperationAccount::DeclaredNone);
    let b = c.binding(&f);
    operation_unavailable(context.authorization().review_operation_context(&b));
    let installed = || InstalledApplicationPolicy {
        operation_review_policy: None,
        originals: f.application.originals.clone(),
        lifetime: f.lifetime.clone(),
        native: f.application.native.clone(),
        selected: f.application.selected.clone(),
    };
    prepared.adopt(installed()).unwrap();
    context.authorization().prepare(&b).unwrap();
    let expected = material(f.context.authorization().review_operation_context(&b));
    assert_eq!(
        material(context.authorization().review_operation_context(&b)),
        expected
    );
    assert!(prepared.adopt(installed()).is_err());
    let mut changed = f.association.candidate().clone();
    changed.original_work.work = id("unretained-material");
    let wrong = policy
        .work_context(
            vec![InputAuthorityAssociation::new(changed).unwrap()].into(),
            scope(&f.session),
        )
        .unwrap();
    operation_unavailable(wrong.authorization().review_operation_context(&b));
    f.lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    denied(context.authorization().review_operation_context(&b));
    assert_eq!(c.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn installed_review_material_refuses_unknown_account_and_unmapped_operation() {
    let f = Fixture::with_targeted_setup().await;
    for c in [
        SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL),
        call(meerkat_core::OperationAccount::Unavailable),
    ] {
        let b = c.binding(&f);
        f.context.authorization().prepare(&b).unwrap();
        operation_unavailable(f.context.authorization().review_operation_context(&b));
        assert_eq!(c.calls.load(Ordering::SeqCst), 0);
    }
    let unknown = SetupBundleCall::with_account(
        "unknown-bundle",
        "unknown-operation",
        Some(meerkat_core::OperationAccount::DeclaredNone),
    );
    assert!(
        f.context
            .authorization()
            .review_operation_context(&unknown.binding(&f))
            .is_err()
    );
    let model = f.model_binding(f.facts.clone());
    assert!(
        f.context
            .authorization()
            .review_operation_context(&model)
            .is_err(),
        "credential identity is not an action account"
    );
}

#[tokio::test]
async fn installed_review_material_refuses_expired_and_revoked_actual_lineage() {
    let f = Fixture::with_targeted_setup().await;
    let c = call(meerkat_core::OperationAccount::DeclaredNone);
    let b = c.binding(&f);
    f.context.authorization().prepare(&b).unwrap();
    material(f.context.authorization().review_operation_context(&b));
    denied(f.application.review_mandate(&f.association, &b, u64::MAX));
    let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } =
        &f.association.candidate().authority_basis
    else {
        panic!("targeted")
    };
    let machine = meerkat_runtime::MeerkatMachine::ephemeral()
        .with_local_grant_authorization(f.source.configuration(
            ScopedInvocation::from_policy(f.application.clone()),
            f.application.clone(),
            None,
        ))
        .unwrap();
    {
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        f.source
            .grants()
            .revoke(
                &meerkat_core::AdmissionGuard::none(),
                &f.application.native.grant_administrator,
                targets[0].lineage.last().unwrap(),
                &mut custody,
            )
            .unwrap();
    }
    denied(f.context.authorization().review_operation_context(&b));
    assert_eq!(c.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn installed_review_material_keeps_provider_source_attribution_and_memory_scope() {
    let f = Fixture::with_operations_review(memory_bounds, false, true).await;
    let component = toolkit_package::Id::new("frontend").unwrap();
    f.lifetime.open_component(&component).unwrap();
    let b = f.memory(f.session.clone(), SourceAuthorizationUse::Read);
    f.context.authorization().prepare(&b).unwrap();
    let m = material(f.context.authorization().review_operation_context(&b));
    assert_eq!(m.action_account(), None);
    assert!(
        m.mandate()
            .unwrap()
            .contains("provider-event source policy")
    );
    assert!(
        m.mandate()
            .unwrap()
            .contains("Source authentication is not a person's approval")
    );
    denied(
        f.context
            .authorization()
            .review_operation_context(&f.memory(SessionId::new(), SourceAuthorizationUse::Read)),
    );
    f.lifetime
        .begin_close_component(&component, HostCloseReason::OperatorDisable)
        .unwrap();
    denied(f.context.authorization().review_operation_context(&b));
}

#[tokio::test]
async fn installed_review_material_accepts_actual_catalog_owner_without_account() {
    let f = Fixture::with_operations(catalog_bounds).await;
    let c = CatalogCall::new("tool_catalog_search");
    let b = c.binding(&f);
    f.context.authorization().prepare(&b).unwrap();
    let m = material(f.context.authorization().review_operation_context(&b));
    assert_eq!(m.action_account(), None);
    assert!(
        m.mandate()
            .unwrap()
            .contains("current native grant-lineage mandate")
    );
}

#[test]
fn native_account_free_target_projection_does_not_infer_missing_account() {
    use super::super::review_material::reviewed_target_account;
    use meerkat_core::{OperationAccount, ToolOperationTarget as T};
    // Typed projection only. These values do not construct dispatch witnesses
    // or prove the independent enrollment/grants needed for actual operations.
    for target in [
        T::PeerDirectoryRead,
        T::PeerSend {
            peer: meerkat_core::comms::PeerId::new(),
            kind: meerkat_core::PeerSendKind::Message,
        },
        T::MemoryOperation {
            scope: meerkat_core::memory::MemoryScopeRef::Session {
                session_id: SessionId::new(),
            },
            operation: meerkat_core::MemoryOperation::Read,
        },
    ] {
        assert_eq!(
            target.account(),
            None,
            "native declares this family account-free"
        );
        assert_eq!(reviewed_target_account(&target).unwrap(), None);
    }
    let unknown = T::RegisteredToolBundleOperation {
        bundle: "unknown".into(),
        operation: "read".into(),
        account: Arc::new(OperationAccount::Unavailable),
    };
    operation_unavailable(reviewed_target_account(&unknown));
}
