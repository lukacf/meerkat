//! Canonical owner checkpoint control with real machine registration witnesses.
//! This does not claim an actual actor build, callback or held Created outcome.
use super::*;
use futures::FutureExt;

#[tokio::test]
async fn inert_source_coordinates_do_not_reclassify_an_existing_registration() {
    use meerkat_authorization_contracts::constraints::ResourceDomain;
    use meerkat_toolkit::host_lifetime::policy::{
        CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
    };
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    pending(&owner, false).await;
    owner.recover().await.unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        std::time::Duration::from_secs(30),
        async {
            let selected = owner
                .select_registration(&realm, &member, &witness)
                .await
                .unwrap()
                .unwrap();
            // Same named Prepared checkpoint as the existing strict route control.
            {
                let _exclusion = owner.exclusion.lock().await;
                let (stored, mut slot) = owner.read().await.unwrap();
                slot.admissions[0].phase = MemberClassAdmissionPhase::Prepared {
                    context: selected.context.clone(),
                    context_generation: selected.context_generation,
                };
                owner
                    .change(&stored, &slot, "fixture-prepared-checkpoint")
                    .await
                    .unwrap();
                owner.activate_current(&slot).unwrap();
            }
            let route = CompiledNativeSourceRoute {
                id: Id::new("project-console").unwrap(),
                origin: CompiledNativeSourceOrigin::LocalConsole {
                    member: member.clone(),
                    requester: principal(),
                    ingress_actor: principal(),
                },
                application_parent: None,
                controller_parent: None,
                logical_executor: principal(),
                represented_subject: None,
                source_authority: principal(),
                authentication_domain: ResourceDomain {
                    authority: principal(),
                    namespace: "login".into(),
                },
                event_domain: ResourceDomain {
                    authority: principal(),
                    namespace: "console".into(),
                },
            };
            owner
                .commit_member_route(
                    admitted.admission_id(),
                    &selected.context,
                    route.clone(),
                    None,
                )
                .await
                .unwrap();
            assert!(
                owner
                    .cold_ordinary_source_coordinates(&route.id)
                    .unwrap()
                    .is_none(),
                "Prepared cannot supply cold Active coordinates"
            );
            owner
                .activate_member(admitted.admission_id(), &witness)
                .await
                .unwrap();
            assert!(
                owner
                    .cold_ordinary_source_coordinates(&route.id)
                    .unwrap()
                    .is_none(),
                "live registration keeps the strict path"
            );
            machine.unregister_session(&session).await.unwrap();
            assert!(
                owner.cold_ordinary_source_coordinates(&route.id).is_err(),
                "ended is not absent"
            );
            let replacement = register(&machine, &session).await;
            owner
                .select_registration(&realm, &member, &replacement)
                .await
                .unwrap()
                .unwrap();
            assert!(
                owner.cold_ordinary_source_coordinates(&route.id).is_err(),
                "selected but unverified is not absent"
            );
            owner
                .verify_registration(&realm, &member, &replacement)
                .await
                .unwrap();
            assert!(
                owner
                    .cold_ordinary_source_coordinates(&route.id)
                    .unwrap()
                    .is_none()
            );
            owner
                .select_registration(&realm, &member, &replacement)
                .await
                .unwrap()
                .unwrap();
            assert!(
                owner.cold_ordinary_source_coordinates(&route.id).is_err(),
                "a reselected generation cannot borrow the previous verification"
            );
            assert!(owner.member_route(&route.id).is_err());
        },
    ))
    .catch_unwind()
    .await;
    // Real machine cleanup, including a panic after replacement registration.
    if machine
        .current_session_registration_witness(&session)
        .await
        .is_some()
    {
        machine.unregister_session(&session).await.unwrap();
    }
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("registration coordinate control timed out"),
    }
}
