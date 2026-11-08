//! Actual pending owner/store controls. Native destination capture and held
//! completion are exercised by the shared public consumer, not fabricated here.
use super::*;
use meerkat_authorization_contracts::{evidence::EvidenceId, work_association::OriginalWorkRef};
use meerkat_mobkit::identity_first::{AgentAddressability, AgentIdentity, DurableAgentSpec};

fn spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse("agent").unwrap(),
        profile: "agent".into(),
        addressability: AgentAddressability::InternalOnly,
        display_name: None,
        labels: Default::default(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

#[tokio::test]
async fn pending_workflow_requires_exact_owner_member_original_and_actual_capture() {
    let (_directory, mut selection) = super::super::tests::fixture();
    super::super::tests::provision_fixture(&mut selection);
    let (selected, policy) = super::tests::inputs(&selection);
    let owner = PreparedInstallationOwner::open(
        selection.deployment_root.clone(),
        selected.clone(),
        operator_setup::OperatorOwnerStartup::Restore,
    )
    .unwrap();
    let mut prepared = owner.prepare(policy).await.unwrap();
    let route = &selected.native_host.routes[0];
    let member = &selected.native_host.members[0].member;
    let original = EvidenceId::new("prepared-g").unwrap();
    let grants = state_directory_in(
        &selection.deployment_root,
        &selected.native_host.realm,
        NativeOwnerStartup::Restore,
    )
    .unwrap()
    .join("grants.jsonl");
    let before = fs::read(&grants).unwrap();
    let _pending = prepared
        .prepare_workflow_member(&route.id, &original, &spec())
        .await
        .unwrap();
    prepared
        .prepare_workflow_member(&route.id, &original, &spec())
        .await
        .unwrap();
    assert!(
        matches!(
            prepared.source.check_prepared_workflow_member_capture(
                member,
                &OriginalWorkRef {
                    authority: route.source_authority.clone(),
                    work: original.clone()
                }
            ),
            Err(crate::original_ingress::OriginalWriterError::Unavailable)
        ),
        "preselection cannot substitute for a real native registration"
    );
    assert!(matches!(
        prepared
            .prepare_workflow_member(
                &route.id,
                &EvidenceId::new("replacement-g").unwrap(),
                &spec()
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    assert!(matches!(
        prepared
            .prepare_workflow_member(
                &toolkit_package::Id::new("other-route").unwrap(),
                &original,
                &spec()
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    let mut public = spec();
    public.addressability = AgentAddressability::Addressable;
    assert!(matches!(
        prepared
            .prepare_workflow_member(&route.id, &original, &public)
            .await,
        Err(HostLifetimeError::Denied)
    ));
    let mut foreign = selected.clone();
    foreign.deployment = toolkit_package::Id::new("foreign-owner").unwrap();
    assert!(matches!(
        prepared
            .originals
            .prepare_workflow_member_for_host(&foreign, &route.id, &original, &spec())
            .await,
        Err(crate::private_originals::PrivateOriginalError::Denied)
    ));
    assert_eq!(
        fs::read(&grants).unwrap(),
        before,
        "preselection issues no authority"
    );
    prepared.finalize_selection().unwrap();
    assert!(
        matches!(
            prepared
                .prepare_workflow_member(&route.id, &original, &spec())
                .await,
            Err(HostLifetimeError::InvalidSelection)
        ),
        "installed owners cannot add a pending reservation"
    );
}
