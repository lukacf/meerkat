use super::*;
use crate::authority::test_support as fixture;
use meerkat_core::auth::PrincipalKind;
use std::collections::BTreeSet;

fn evidence() -> SetupRequestEvidence {
    let requester = fixture::principal(PrincipalKind::Human, "person");
    let target = fixture::target("agent", &requester);
    SetupRequestEvidence::new(
        fixture::association(requester, target, "original", "event"),
        SetupProposalConstraints {
            tenant: Id::new("tenant").unwrap(),
            deployment: Id::new("deployment").unwrap(),
            policy_revision: Revision::new("policy").unwrap(),
            issued_at_ms: 10,
            expires_at_ms: 100,
            ceiling: Ceiling {
                actions: BTreeSet::new(),
                services: BTreeSet::new(),
                frontends: BTreeSet::new(),
                output_audiences: BTreeSet::new(),
            },
        },
    )
    .unwrap()
}
fn context<'a>(
    e: &'a SetupRequestEvidence,
    native: &'a WorkAuthorizationContext,
) -> SetupUseContext<'a> {
    SetupUseContext {
        tenant: &e.constraints.tenant,
        deployment: &e.constraints.deployment,
        realm: &e.association.candidate().ingress_namespace.realm,
        target: &e.association.candidate().target,
        policy_revision: &e.constraints.policy_revision,
        now_ms: 20,
        authorization: native,
    }
}

#[test]
fn legacy_signed_roots_and_unknown_versions_require_migration() {
    for version in [0, 2, 3, u16::MAX] {
        let legacy = format!(
            r#"{{"version":{version},"claims":{{}},"signature":"{}"}}"#,
            "0".repeat(64)
        );
        let error = serde_json::from_str::<SetupRequestEvidence>(&legacy).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("require native ingress migration")
        );
    }
    let forged_version = r#"{"version":1,"claims":{},"signature":"old-root"}"#;
    assert!(serde_json::from_str::<SetupRequestEvidence>(forged_version).is_err());
}

#[test]
fn native_evidence_roundtrip_preserves_qualified_identity_without_permission() {
    let e = evidence();
    let bytes = serde_json::to_vec(&e).unwrap();
    let decoded: SetupRequestEvidence = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, e);
    assert_eq!(
        decoded.association().candidate().requester,
        e.association().candidate().requester
    );
    let native = fixture::current(e.association());
    assert!(decoded.check_context(&context(&decoded, &native)).is_ok());
    // The empty setup proposal ceiling cannot authorize even a proposal. The
    // native fixture also refuses every operation regardless of decoding.
    assert_eq!(
        decoded.check_proposal(
            Action::FindAndInstall,
            &Id::new("agent").unwrap(),
            &context(&decoded, &native)
        ),
        Err(SetupContextError::Denied)
    );
    let duplicate = String::from_utf8(bytes).unwrap().replacen(
        r#""version":1"#,
        r#""version":1,"version":1"#,
        1,
    );
    assert!(serde_json::from_str::<SetupRequestEvidence>(&duplicate).is_err());
}

#[test]
fn retained_owner_json_digest_is_distinct_from_binary_canonical_encoding() {
    let e = evidence();
    let native = fixture::current(e.association());
    let owner_digest = ContentDigest::of_bytes(&serde_json::to_vec(e.association()).unwrap());
    let binary_digest = ContentDigest::of_bytes(&e.association().canonical_bytes().unwrap());
    assert_ne!(owner_digest, binary_digest);
    assert_eq!(
        native.retained_work().unwrap().contributors()[0].association_digest,
        owner_digest.as_str()
    );
    assert!(e.check_context(&context(&e, &native)).is_ok());
    let mut changed = e.association().candidate().clone();
    changed.original_authentication = fixture::event("different-authentication");
    let changed = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(changed).unwrap(),
        e.constraints.clone(),
    )
    .unwrap();
    assert_eq!(
        changed.check_context(&context(&changed, &native)),
        Err(SetupContextError::WrongContext)
    );
}

#[test]
fn decoding_evidence_does_not_recreate_current_native_work() {
    let e = evidence();
    let other = evidence();
    let mut candidate = other.association().candidate().clone();
    candidate.requester = fixture::principal(PrincipalKind::Human, "another-person");
    let foreign = InputAuthorityAssociation::new(candidate).unwrap();
    let native = fixture::current(&foreign);
    assert_eq!(
        e.check_context(&context(&e, &native)),
        Err(SetupContextError::WrongContext)
    );
}
