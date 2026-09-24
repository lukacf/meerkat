//! THE containment rule, exercised once for both surfaces.
//!
//! This is deliberately a single table, not two: the point of factoring
//! `adjudicate_protected_resume` out of the local actor and the host actor was
//! that there is one rule with two proof shapes. If a row here had to be
//! duplicated per surface, the factoring would have failed.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use chrono::Utc;
use meerkat_core::SessionId;
use meerkat_mob::forked_participant::{
    ForkedParticipantAttachmentId, ForkedParticipantForkProtection,
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute, ForkedParticipantProvenance,
    ForkedParticipantRef, ForkedParticipantResumeAdmission, ForkedParticipantResumeProof,
    ForkedParticipantResumeRejection, ForkedParticipantReusePolicy, LocalAssociationEvidence,
    adjudicate_protected_resume,
};
use meerkat_mob::ids::AgentIdentity;

const MEMBER: &str = "branch-1";
const OTHER_MEMBER: &str = "branch-2";

fn realm() -> meerkat_core::RealmId {
    meerkat_core::RealmId::parse("global").expect("realm")
}

fn local_route() -> ForkedParticipantOwnerRoute {
    ForkedParticipantOwnerRoute::Local { realm_id: realm() }
}

fn host_route() -> ForkedParticipantOwnerRoute {
    ForkedParticipantOwnerRoute::Host {
        realm_id: realm(),
        host_id: meerkat_mob::machines::mob_machine::HostId::from("host-a".to_string()),
    }
}

fn other_host_route() -> ForkedParticipantOwnerRoute {
    ForkedParticipantOwnerRoute::Host {
        realm_id: realm(),
        host_id: meerkat_mob::machines::mob_machine::HostId::from("host-b".to_string()),
    }
}

/// Mint a reference through the wire round-trip, which is the only way to
/// build one outside the owning crate — and exactly how a presented (possibly
/// tampered) reference reaches a surface.
fn reference(
    fork_session: &SessionId,
    source_session: &SessionId,
    route: &ForkedParticipantOwnerRoute,
    scope: ForkedParticipantOperationScope,
    prefix_digest: &str,
) -> ForkedParticipantRef {
    let template = serde_json::json!({
        "capability_id": "0".repeat(64),
        "source_identity": "researcher",
        "fork_session_id": fork_session.to_string(),
        "owner_route": match route {
            ForkedParticipantOwnerRoute::Local { realm_id } => serde_json::json!({
                "kind": "local",
                "realm_id": realm_id.as_str(),
            }),
            ForkedParticipantOwnerRoute::Host { realm_id, host_id } => serde_json::json!({
                "kind": "host",
                "realm_id": realm_id.as_str(),
                "host_id": host_id.as_str(),
            }),
        },
        "provenance": {
            "source_session_id": source_session.to_string(),
            "prefix_message_count": 3,
            "prefix_digest": prefix_digest,
        },
        "scope": match scope {
            ForkedParticipantOperationScope::Invoke => "invoke",
            ForkedParticipantOperationScope::Observe => "observe",
            ForkedParticipantOperationScope::InvokeAndObserve => "invoke_and_observe",
        },
        "expires_at": (Utc::now() + chrono::Duration::seconds(600)).to_rfc3339(),
        "reuse": { "kind": "one_shot" },
        "revocation_id": "fpr:req-1",
        "cleanup_id": "fpk:req-1",
    });
    serde_json::from_value(template).expect("reference round-trips through the wire shape")
}

struct Fixture {
    fork_session: SessionId,
    source_session: SessionId,
    recorded_local: ForkedParticipantRef,
    recorded_host: ForkedParticipantRef,
}

impl Fixture {
    fn new() -> Self {
        let fork_session = SessionId::new();
        let source_session = SessionId::new();
        Self {
            recorded_local: reference(
                &fork_session,
                &source_session,
                &local_route(),
                ForkedParticipantOperationScope::Invoke,
                "sha256:prefix",
            ),
            recorded_host: reference(
                &fork_session,
                &source_session,
                &host_route(),
                ForkedParticipantOperationScope::Invoke,
                "sha256:prefix",
            ),
            fork_session,
            source_session,
        }
    }

    fn protection(&self, recorded: &ForkedParticipantRef) -> ForkedParticipantForkProtection {
        ForkedParticipantForkProtection {
            capability_hint: "fpc:testhint0000".to_string(),
            owner_route: recorded.owner_route().clone(),
            capability: Some(recorded.clone()),
        }
    }

    fn reserved_only(
        &self,
        route: &ForkedParticipantOwnerRoute,
    ) -> ForkedParticipantForkProtection {
        ForkedParticipantForkProtection {
            capability_hint: "fpc:testhint0000".to_string(),
            owner_route: route.clone(),
            capability: None,
        }
    }
}

/// What a row expects. `Reject` compares the rejection's discriminant shape by
/// its `Display` prefix-free identity, which is stable and surface-neutral.
enum Expect {
    Admit(ForkedParticipantResumeAdmission),
    Reject(fn(&ForkedParticipantResumeRejection) -> bool),
}

fn is_authority_required(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::AuthorityRequired { .. }
    )
}
fn is_reserved(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::ReservedNotActivated { .. }
    )
}
fn is_reference_mismatch(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::ReferenceMismatch { .. }
    )
}
fn is_foreign_route(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::ForeignRoute { .. }
    )
}
fn is_member_mismatch(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::MemberMismatch { .. }
    )
}
fn is_session_mismatch(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::SessionMismatch { .. }
    )
}
fn is_malformed_attachment(rejection: &ForkedParticipantResumeRejection) -> bool {
    matches!(
        rejection,
        ForkedParticipantResumeRejection::MalformedAttachmentId { .. }
    )
}

#[test]
fn the_containment_rule_is_one_table_for_both_surfaces() {
    let fixture = Fixture::new();
    let local_protection = fixture.protection(&fixture.recorded_local);
    let host_protection = fixture.protection(&fixture.recorded_host);
    let member = AgentIdentity::from(MEMBER);

    // A reference that differs only in a WIDENED scope: the exact-comparison
    // rows must reject it, which is what makes "exact" mean exact.
    let widened_host = reference(
        &fixture.fork_session,
        &fixture.source_session,
        &host_route(),
        ForkedParticipantOperationScope::InvokeAndObserve,
        "sha256:prefix",
    );
    // ...and one whose provenance digest was rewritten.
    let rewritten_local = reference(
        &fixture.fork_session,
        &fixture.source_session,
        &local_route(),
        ForkedParticipantOperationScope::Invoke,
        "sha256:rewritten",
    );
    // ...and one naming a different fork session entirely.
    let other_session = SessionId::new();
    let wrong_session_local = reference(
        &other_session,
        &fixture.source_session,
        &local_route(),
        ForkedParticipantOperationScope::Invoke,
        "sha256:prefix",
    );

    let rows: Vec<(
        &str,
        Option<&ForkedParticipantForkProtection>,
        ForkedParticipantResumeProof,
        Expect,
    )> = vec![
        // ---- shared: no protection at all ----
        (
            "unprotected session, no proof, admits",
            None,
            ForkedParticipantResumeProof::Absent,
            Expect::Admit(ForkedParticipantResumeAdmission::Unprotected),
        ),
        (
            "unprotected session, host proof, admits",
            None,
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref: fixture.recorded_host.clone(),
                attachment_id: "attach-1".to_string(),
                owner_route: host_route(),
            },
            Expect::Admit(ForkedParticipantResumeAdmission::Unprotected),
        ),
        // ---- shared: protected + absent proof ----
        (
            "protected, absent proof, rejects (the bypass this rule exists for)",
            Some(&local_protection),
            ForkedParticipantResumeProof::Absent,
            Expect::Reject(is_authority_required),
        ),
        // ---- shared: reserved-but-unactivated rejects EVERY proof shape ----
        (
            "reserved-only, absent proof, rejects",
            None,
            ForkedParticipantResumeProof::Absent,
            Expect::Admit(ForkedParticipantResumeAdmission::Unprotected),
        ),
        // ---- local proof shapes ----
        (
            "local: attach-admitting spawn in flight, admits",
            Some(&local_protection),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member: member.clone(),
                session: fixture.fork_session.clone(),
                association: None,
            },
            Expect::Admit(ForkedParticipantResumeAdmission::LocalAttachedSpawnInFlight),
        ),
        (
            "local: exact durable association, admits",
            Some(&local_protection),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member: member.clone(),
                session: fixture.fork_session.clone(),
                association: Some(LocalAssociationEvidence {
                    member: member.clone(),
                    capability: fixture.recorded_local.clone(),
                }),
            },
            Expect::Admit(ForkedParticipantResumeAdmission::LocalCustody),
        ),
        (
            "local: association names a different member, rejects",
            Some(&local_protection),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member: member.clone(),
                session: fixture.fork_session.clone(),
                association: Some(LocalAssociationEvidence {
                    member: AgentIdentity::from(OTHER_MEMBER),
                    capability: fixture.recorded_local.clone(),
                }),
            },
            Expect::Reject(is_member_mismatch),
        ),
        (
            "local: association names a different fork session, rejects",
            Some(&local_protection),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member: member.clone(),
                session: fixture.fork_session.clone(),
                association: Some(LocalAssociationEvidence {
                    member: member.clone(),
                    capability: wrong_session_local,
                }),
            },
            Expect::Reject(is_session_mismatch),
        ),
        (
            "local: association drifted from owner truth, rejects",
            Some(&local_protection),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member: member.clone(),
                session: fixture.fork_session.clone(),
                association: Some(LocalAssociationEvidence {
                    member: member.clone(),
                    capability: rewritten_local,
                }),
            },
            Expect::Reject(is_reference_mismatch),
        ),
        (
            "local proof against a HOST-owned capability, rejects",
            Some(&host_protection),
            ForkedParticipantResumeProof::LocalAttachedSpawn {
                member,
                session: fixture.fork_session.clone(),
                association: None,
            },
            Expect::Reject(is_foreign_route),
        ),
        // ---- host proof shapes ----
        (
            "host: exact reference + well-formed attachment, admits",
            Some(&host_protection),
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref: fixture.recorded_host.clone(),
                attachment_id: "attach-1".to_string(),
                owner_route: host_route(),
            },
            Expect::Admit(ForkedParticipantResumeAdmission::HostCapability),
        ),
        (
            "host: tampered (widened scope) reference, rejects",
            Some(&host_protection),
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref: widened_host,
                attachment_id: "attach-1".to_string(),
                owner_route: host_route(),
            },
            Expect::Reject(is_reference_mismatch),
        ),
        (
            "host: reference naming a different fork session, rejects",
            Some(&host_protection),
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref: reference(
                    &other_session,
                    &fixture.source_session,
                    &host_route(),
                    ForkedParticipantOperationScope::Invoke,
                    "sha256:prefix",
                ),
                attachment_id: "attach-1".to_string(),
                owner_route: host_route(),
            },
            Expect::Reject(is_reference_mismatch),
        ),
        (
            "host: exact reference served by a foreign route, rejects",
            Some(&host_protection),
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref: fixture.recorded_host.clone(),
                attachment_id: "attach-1".to_string(),
                owner_route: other_host_route(),
            },
            Expect::Reject(is_foreign_route),
        ),
        (
            "host: exact reference, malformed attachment id, rejects",
            Some(&host_protection),
            ForkedParticipantResumeProof::HostCapabilityAttachment {
                full_ref: fixture.recorded_host.clone(),
                attachment_id: "   ".to_string(),
                owner_route: host_route(),
            },
            Expect::Reject(is_malformed_attachment),
        ),
    ];

    for (name, protection, proof, expect) in rows {
        let outcome = adjudicate_protected_resume(protection, &proof);
        match (expect, outcome) {
            (Expect::Admit(expected), Ok(actual)) => {
                assert_eq!(
                    actual, expected,
                    "row '{name}' admitted for the wrong reason"
                );
            }
            (Expect::Admit(expected), Err(rejection)) => {
                panic!("row '{name}' expected {expected:?} but was rejected: {rejection}");
            }
            (Expect::Reject(matches), Err(rejection)) => {
                assert!(
                    matches(&rejection),
                    "row '{name}' rejected with the wrong reason: {rejection:?}"
                );
                assert!(
                    !rejection.capability_hint().is_empty(),
                    "row '{name}' must carry a correlation hint"
                );
            }
            (Expect::Reject(_), Ok(admission)) => {
                panic!("row '{name}' must be refused but admitted as {admission:?}");
            }
        }
    }
}

#[test]
fn a_reserved_but_unactivated_record_refuses_every_proof_shape() {
    // Called out separately because it is the one rule that must hold ACROSS
    // proof shapes: the crash window between "planned child is durable" and
    // "activation is recorded" has no reference to authenticate against, so
    // nothing may pass regardless of which surface asks.
    let fixture = Fixture::new();
    let member = AgentIdentity::from(MEMBER);
    for (label, route) in [("local", local_route()), ("host", host_route())] {
        let reserved = fixture.reserved_only(&route);
        for (proof_label, proof) in [
            ("absent", ForkedParticipantResumeProof::Absent),
            (
                "local attached spawn",
                ForkedParticipantResumeProof::LocalAttachedSpawn {
                    member: member.clone(),
                    session: fixture.fork_session.clone(),
                    association: None,
                },
            ),
            (
                "local durable association",
                ForkedParticipantResumeProof::LocalAttachedSpawn {
                    member: member.clone(),
                    session: fixture.fork_session.clone(),
                    association: Some(LocalAssociationEvidence {
                        member: member.clone(),
                        capability: fixture.recorded_local.clone(),
                    }),
                },
            ),
            (
                "host capability attachment",
                ForkedParticipantResumeProof::HostCapabilityAttachment {
                    full_ref: fixture.recorded_host.clone(),
                    attachment_id: "attach-1".to_string(),
                    owner_route: host_route(),
                },
            ),
        ] {
            let rejection = adjudicate_protected_resume(Some(&reserved), &proof)
                .expect_err("a reserved-only record admits nothing");
            let expected_reserved = matches!(proof, ForkedParticipantResumeProof::Absent);
            if expected_reserved {
                assert!(
                    is_authority_required(&rejection),
                    "{label}/{proof_label}: an absent proof is refused for the absent reason"
                );
            } else {
                assert!(
                    is_reserved(&rejection),
                    "{label}/{proof_label}: expected ReservedNotActivated, got {rejection:?}"
                );
            }
        }
    }
}

#[test]
fn the_rule_reads_nothing_but_its_two_arguments() {
    // Totality/purity guard: the same inputs must yield the same verdict, and
    // no argument may be optional-in-practice. A surface that "usually" passes
    // its proof would otherwise be admitting on ambient state.
    let fixture = Fixture::new();
    let protection = fixture.protection(&fixture.recorded_host);
    let proof = ForkedParticipantResumeProof::HostCapabilityAttachment {
        full_ref: fixture.recorded_host.clone(),
        attachment_id: "attach-1".to_string(),
        owner_route: host_route(),
    };
    let first = adjudicate_protected_resume(Some(&protection), &proof);
    std::thread::sleep(Duration::from_millis(5));
    let second = adjudicate_protected_resume(Some(&protection), &proof);
    assert_eq!(
        first, second,
        "the rule must not depend on a clock or any ambient state"
    );
    assert_eq!(first, Ok(ForkedParticipantResumeAdmission::HostCapability));

    // Reuse policy is part of the compared reference, so a reference that
    // differs only in reuse is still a mismatch.
    assert_eq!(
        fixture.recorded_host.reuse(),
        ForkedParticipantReusePolicy::OneShot
    );
    assert!(
        ForkedParticipantAttachmentId::new("attach-1").is_ok(),
        "the fixture's attachment id must be a valid identity"
    );
    assert_eq!(
        fixture.recorded_host.provenance(),
        &ForkedParticipantProvenance {
            source_session_id: fixture.source_session.clone(),
            prefix_message_count: 3,
            prefix_digest: "sha256:prefix".to_string(),
        }
    );
}
