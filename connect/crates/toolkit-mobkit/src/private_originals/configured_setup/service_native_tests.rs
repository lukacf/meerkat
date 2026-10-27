//! Native attenuation and the optional metadata-profile classifier. The
//! classifier assertions do not claim cross-audience helper execution.
use super::*;
use futures::FutureExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn setup_child_observation_preserves_bounded_native_parent_lifetime() {
    let fixture = crate::private_originals::ContinuationFixture::for_service_input().await;
    let checked = std::panic::AssertUnwindSafe(async {
        let source = &fixture.source;
        let facts = source
            .configured_setup(fixture.original_reference())
            .unwrap();
        let now = source.owner.clock().now().unwrap().unix_ms;
        let mut bounds = facts.parent_bounds();
        bounds.lifetime = LifetimeRestriction::window(now.saturating_sub(1_000), now + 30_000);
        let root = fixture
            .lifetime()
            .selected_policy()
            .native_host
            .as_ref()
            .unwrap()
            .routes[0]
            .controller_parent
            .as_ref()
            .unwrap()[0]
            .root_authority
            .clone();
        let parent = source
            .owner
            .grants()
            .issue_root(
                &root,
                EvidenceId::new("bounded-warm-test-parent").unwrap(),
                facts.application.clone(),
                facts.route.represented_subject.clone(),
                bounds.clone(),
            )
            .unwrap();
        let mut requested = facts.child_bounds(&fixture.session);
        requested.lifetime = LifetimeRestriction::window(0, now + 60_000);
        let selected = ServiceSetupChild::new(
            crate::installed_host::operator_setup::setup_targets::find_target(),
            parent.clone(),
            EvidenceId::new("bounded-warm-test-child").unwrap(),
            bounds.clone(),
            requested.clone(),
        )
        .unwrap();
        assert_eq!(selected.expected.lifetime, bounds.lifetime);
        assert_ne!(
            selected.expected.lifetime, requested.lifetime,
            "the task deadline does not replace an earlier parent expiry or not-before bound"
        );
        let leaf = source
            .owner
            .grants()
            .issue_child(
                &facts.application,
                &parent,
                selected.id.clone(),
                facts.route.logical_executor.clone(),
                requested,
            )
            .unwrap();
        let observation = selected
            .observe(&source.owner.grants(), &facts, leaf.clone())
            .unwrap();
        assert!(selected.matches(&observation));
        assert_eq!(observation.restrictions.lifetime, bounds.lifetime);
        let mut wrong = observation.clone();
        wrong.restrictions.lifetime = LifetimeRestriction::window(0, now + 60_000);
        assert!(!selected.matches(&wrong));
        let machine = source.machine().unwrap();
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        source
            .owner
            .grants()
            .revoke(&facts.application, &leaf, &mut custody)
            .unwrap();
        drop(custody);
        let before = std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap();
        ServiceSetupCleanup {
            issuer: Some(facts.application.clone()),
            children: vec![observation],
            unresolved: false,
        }
        .revoke(source)
        .unwrap();
        assert_eq!(
            std::fs::read(fixture.source_path.with_file_name("grants.jsonl")).unwrap(),
            before,
            "a retained exact observation supports idempotent already-revoked cleanup"
        );
        source
            .owner
            .grants()
            .resolve_lineage(
                &[parent],
                &facts.application,
                facts.route.represented_subject.as_ref(),
            )
            .unwrap()
            .check_current()
            .unwrap();

        // Scope classification only. Actual same-executor public preparation
        // is covered by the helper fixture; no runtime is fabricated here.
        assert!(supports_warm_metadata(
            &facts.route.logical_executor,
            &facts.member.audience,
            &facts.application,
            &facts.member.audience
        ));
        assert!(!supports_warm_metadata(
            &facts.route.logical_executor,
            &facts.member.audience,
            &facts.route.logical_executor,
            &facts.member.audience
        ));
        let other_audience = AudienceRef::Principal {
            principal: facts.application.clone(),
        };
        assert!(other_audience != facts.member.audience);
        assert!(!supports_warm_metadata(
            &facts.route.logical_executor,
            &facts.member.audience,
            &facts.application,
            &other_audience
        ));
        assert_eq!(fixture.inputs().await, Ok(0));
    })
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(payload) = checked {
        std::panic::resume_unwind(payload);
    }
}
