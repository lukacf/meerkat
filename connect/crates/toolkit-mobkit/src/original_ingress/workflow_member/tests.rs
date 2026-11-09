//! Canonical owner selection checks only. Decoded workflow addresses below are
//! inert identity data; no controller, registration, admission or grant is made.
use super::*;
use crate::original_ingress::tests::{initialized, principal};

fn reference(name: &str) -> SetupWorkflowRef {
    serde_json::from_value(serde_json::json!({
        "key": {"realm":"realm", "owner":"application", "id":name},
        "terms_digest": toolkit_package::ContentDigest::of_bytes(name.as_bytes()),
    }))
    .unwrap()
}
fn selection(name: &str, reference: Option<SetupWorkflowRef>) -> SelectedWorkflowMember {
    SelectedWorkflowMember {
        class_admission: None,
        reference,
        member: MobMemberBinding {
            mob_id: "workflow".into(),
            role: "identity".into(),
            member: name.into(),
        },
        original: OriginalWorkRef {
            authority: principal("source"),
            work: EvidenceId::new(name).unwrap(),
        },
        realm: meerkat_core::mob_realm_id("workflow").unwrap(),
        identity: meerkat_mobkit::identity_first::AgentIdentity::parse(name).unwrap(),
        captured: None,
    }
}

#[tokio::test]
async fn pending_member_binds_once_and_cannot_alias_another_workflow() {
    let directory = tempfile::tempdir().unwrap();
    let owner = initialized(&directory.path().join("grants"));
    owner
        .select_workflow_member(selection("g-a", None), || Ok(()), || Ok(()))
        .await
        .unwrap();
    owner
        .select_workflow_member(selection("g-b", Some(reference("b"))), || Ok(()), || Ok(()))
        .await
        .unwrap();
    assert!(matches!(
        owner
            .select_workflow_member(
                selection("g-a", Some(reference("b"))),
                || Ok(()),
                || panic!("conflict must precede host binding")
            )
            .await,
        Err(OriginalWriterError::Conflict)
    ));
    assert!(
        owner.workflow_members.lock().unwrap()[0]
            .reference
            .is_none()
    );
    owner
        .select_workflow_member(selection("g-a", Some(reference("a"))), || Ok(()), || Ok(()))
        .await
        .unwrap();
    owner
        .select_workflow_member(selection("g-a", Some(reference("a"))), || Ok(()), || Ok(()))
        .await
        .unwrap();
    for changed in [None, Some(reference("replacement"))] {
        assert!(matches!(
            owner
                .select_workflow_member(
                    selection("g-a", changed),
                    || Ok(()),
                    || panic!("rebind must precede host binding")
                )
                .await,
            Err(OriginalWriterError::Conflict)
        ));
    }
    let member = selection("g-a", None);
    assert!(
        matches!(
            owner.check_prepared_workflow_member_capture(&member.member, &member.original),
            Err(OriginalWriterError::Unavailable)
        ),
        "reference binding is not native capture"
    );
    let rows = owner.workflow_members.lock().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].reference, Some(reference("a")));
    assert_eq!(rows[1].reference, Some(reference("b")));
    assert!(rows.iter().all(|row| row.captured.is_none()));
}
