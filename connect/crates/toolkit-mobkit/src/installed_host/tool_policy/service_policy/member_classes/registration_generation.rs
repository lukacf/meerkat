//! Actual Core scoped binding and canonical selection across registrations.
//! The closed governed machine admits no input or operation. These controls
//! prove destination recovery, not actor restoration or retained-input replay.
use super::*;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_runtime::{LogicalRuntimeId, RuntimeDriverError};

fn target(
    selected: &MemberDestinationSelection,
    witness: &RuntimeSessionRegistrationWitness,
) -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: selected.logical_owner.clone(),
        logical_runtime: EvidenceId::new(
            LogicalRuntimeId::for_session(witness.session_id()).to_string(),
        )
        .unwrap(),
        context: selected.context.clone(),
        context_generation: selected.context_generation,
        audience: selected.audience.clone(),
    }
}

#[tokio::test]
async fn new_native_registration_rebinds_exact_retained_destination() {
    let (directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let grants =
        crate::original_ingress::tests::initialized(&directory.path().join("native-grants"));
    let machine = crate::original_ingress::tests::machine(&grants);
    let session = meerkat_core::SessionId::new();
    let first = register(&machine, &session).await;
    let selected = owner
        .select_registration(&realm, &member, &first)
        .await
        .unwrap()
        .unwrap();
    let historical_target = target(&selected, &first);
    machine
        .bind_scoped_context(&first, &historical_target)
        .await
        .unwrap();
    let authority = owner.read().await.unwrap().1.current;

    machine.unregister_session(&session).await.unwrap();
    assert!(first.has_ended());
    let replacement = register(&machine, &session).await;
    assert_ne!(
        replacement, first,
        "Core minted a different actual registration"
    );
    assert!(
        owner
            .expected_registration(&realm, &member, &replacement)
            .is_err()
    );
    let recovered = owner
        .select_registration(&realm, &member, &replacement)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target(&recovered, &replacement),
        historical_target,
        "registration replacement must not retarget retained work"
    );
    machine
        .bind_scoped_context(&replacement, &historical_target)
        .await
        .unwrap();
    assert_eq!(
        owner
            .expected_registration(&realm, &member, &replacement)
            .unwrap(),
        Some(recovered)
    );
    assert!(
        owner
            .expected_registration(&realm, &member, &first)
            .is_err()
    );
    assert!(
        machine
            .bind_scoped_context(&first, &historical_target)
            .await
            .is_err(),
        "the old witness cannot bind through the replacement"
    );
    assert_eq!(owner.read().await.unwrap().1.current, authority);
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn same_native_registration_repair_advances_actual_bound_generation() {
    let (directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let grants =
        crate::original_ingress::tests::initialized(&directory.path().join("native-grants"));
    let machine = crate::original_ingress::tests::machine(&grants);
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    let selected = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    let historical_target = target(&selected, &witness);
    machine
        .bind_scoped_context(&witness, &historical_target)
        .await
        .unwrap();
    assert!(matches!(
        machine
            .bind_scoped_context(&witness, &historical_target)
            .await,
        Err(RuntimeDriverError::ScopedContextGenerationNotAdvanced { .. })
    ));
    let repaired = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repaired.context, selected.context);
    assert_eq!(repaired.context_generation, selected.context_generation + 1);
    machine
        .bind_scoped_context(&witness, &target(&repaired, &witness))
        .await
        .unwrap();
    assert!(
        matches!(
            machine
                .bind_scoped_context(&witness, &historical_target)
                .await,
            Err(RuntimeDriverError::ScopedContextGenerationNotAdvanced { .. })
        ),
        "repair cannot make the historical target current again"
    );
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn reopened_policy_keeps_target_but_native_gate_decides_existing_binding() {
    let (directory, policy, owner, rule) = fixture().await;
    let canonical = policy.compiled().canonical_json().unwrap();
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let grants =
        crate::original_ingress::tests::initialized(&directory.path().join("native-grants"));
    let machine = crate::original_ingress::tests::machine(&grants);
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    let selected = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    let historical_target = target(&selected, &witness);
    machine
        .bind_scoped_context(&witness, &historical_target)
        .await
        .unwrap();
    let weak = Arc::downgrade(&owner);
    drop(admitted);
    drop(owner);
    drop(policy);
    assert!(
        weak.upgrade().is_none(),
        "no cached selection crosses owner reopen"
    );

    let policy = PreparedInstalledToolPolicy::from_canonical(&canonical).unwrap();
    let owner = policy
        .bind_service_owner(
            Arc::new(
                toolkit_store::SqliteStore::open(directory.path().join("state.sqlite3")).unwrap(),
            ),
            StoreScope {
                realm: realm.to_string(),
                owner: "deployment".into(),
            },
            principal(),
            Some(rule.clone()),
            vec![rule.class.clone()],
        )
        .await
        .unwrap();
    let reopened = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target(&reopened, &witness), historical_target);
    assert!(
        matches!(
            machine
                .bind_scoped_context(&witness, &historical_target)
                .await,
            Err(RuntimeDriverError::ScopedContextGenerationNotAdvanced { .. })
        ),
        "an absent cache cannot assert that this actual registration is unbound"
    );
    let repair = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(repair.context_generation, reopened.context_generation + 1);
    machine
        .bind_scoped_context(&witness, &target(&repair, &witness))
        .await
        .unwrap();
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn sqlite_policy_reopen_and_new_native_registration_keep_exact_target() {
    let (directory, policy, owner, rule) = fixture().await;
    let canonical = policy.compiled().canonical_json().unwrap();
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let grants =
        crate::original_ingress::tests::initialized(&directory.path().join("native-grants"));
    let machine = crate::original_ingress::tests::machine(&grants);
    let session = meerkat_core::SessionId::new();
    let first = register(&machine, &session).await;
    let selected = owner
        .select_registration(&realm, &member, &first)
        .await
        .unwrap()
        .unwrap();
    let historical_target = target(&selected, &first);
    machine
        .bind_scoped_context(&first, &historical_target)
        .await
        .unwrap();
    let before = owner.read().await.unwrap().1;
    machine.unregister_session(&session).await.unwrap();
    let weak = Arc::downgrade(&owner);
    drop(admitted);
    drop(owner);
    drop(policy);
    assert!(
        weak.upgrade().is_none(),
        "actual canonical owner and SQLite handle closed"
    );

    let policy = PreparedInstalledToolPolicy::from_canonical(&canonical).unwrap();
    let owner = policy
        .bind_service_owner(
            Arc::new(
                toolkit_store::SqliteStore::open(directory.path().join("state.sqlite3")).unwrap(),
            ),
            StoreScope {
                realm: realm.to_string(),
                owner: "deployment".into(),
            },
            principal(),
            Some(rule.clone()),
            vec![rule.class.clone()],
        )
        .await
        .unwrap();
    let replacement = register(&machine, &session).await;
    assert_ne!(first, replacement);
    assert!(
        owner
            .expected_registration(&realm, &member, &replacement)
            .is_err()
    );
    let recovered = owner
        .select_registration(&realm, &member, &replacement)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target(&recovered, &replacement), historical_target);
    machine
        .bind_scoped_context(&replacement, &historical_target)
        .await
        .unwrap();
    let after = owner.read().await.unwrap().1;
    assert_eq!(
        after.admissions, before.admissions,
        "neither the declaration nor its target changed on real owner reopen"
    );
    assert_eq!(after.current, before.current);
    assert!(
        owner
            .expected_registration(&realm, &member, &first)
            .is_err()
    );
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn mismatched_cached_selection_is_unavailable_without_consuming_generation() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    let selected = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    let before = owner.read().await.unwrap().0;
    // These are deliberately inconsistent process observations, not native
    // authority or replacements for the actual canonical admission.
    for wrong_admission in [true, false] {
        {
            let mut cache = owner.member_registrations.lock().unwrap();
            let cached = cache.get_mut(&member.member).unwrap();
            cached.admission = if wrong_admission {
                Id::new("another-admission").unwrap()
            } else {
                admitted.admission_id().clone()
            };
            cached.destination.context_generation =
                selected.context_generation + u64::from(!wrong_admission);
        }
        assert!(matches!(
            owner.select_registration(&realm, &member, &witness).await,
            Err(HostLifetimeError::Unavailable)
        ));
        assert_eq!(
            owner.read().await.unwrap().0,
            before,
            "stale cache cannot allocate or select a current destination"
        );
    }
    machine.unregister_session(&session).await.unwrap();
}
