//! Rows of the durable decision vocabulary: computed subject digests,
//! canonical parts and presentation completeness, derived ids, evidence
//! digests, principal keys, tokens, and the in-memory store's conditional
//! writes and pages.

use std::num::NonZeroU32;

use sha2::{Digest, Sha256};

use super::parts::part_ranges;
use super::*;
use crate::approval::consent::AuthenticatedApprover;
use crate::approval::{ApprovalAssurance, ApprovalStoreError};
use crate::{PrincipalKind, PrincipalRef, SessionId, TrustDomainId};

fn domain(value: &str) -> TrustDomainId {
    TrustDomainId::new(value).expect("trust domain")
}

fn principal(kind: PrincipalKind, id: &str, trust_domain: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, id, domain(trust_domain)).expect("qualified principal")
}

fn key(kind: PrincipalKind, id: &str) -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(&principal(kind, id, "example.org")).expect("principal key")
}

fn reference(value: &str) -> DecisionReference {
    DecisionReference::new(value).expect("reference")
}

fn rendering(text: &str) -> DecisionRendering {
    DecisionRendering::new(text).expect("rendering")
}

fn approver_id() -> ApproverId {
    ApproverId::new("team:owners").expect("approver id")
}

fn hold_request(
    rendering: DecisionRendering,
    expected_subject_digest: Option<Sha256Digest>,
) -> DecisionHoldRequest {
    let request = DecisionHoldRequest::new(
        DecisionKind::new("release").expect("kind"),
        DecisionSubjectKey::new("artifact:build-7").expect("subject key"),
        DecisionOperationId::new("operation-7").expect("operation id"),
        Sha256Digest::of(b"{\"build\":7}"),
        DecisionRequestBody::from_value(&serde_json::json!({"build": 7})).expect("request body"),
        key(PrincipalKind::PersonalAgent, "builder"),
        approver_id(),
        60_000,
        rendering,
        DecisionRenderer::new("render.v1").expect("renderer"),
    );
    match expected_subject_digest {
        Some(digest) => request.with_expected_subject_digest(digest),
        None => request,
    }
}

fn attested(approve: bool) -> AttestedDecision {
    let approver = AuthenticatedApprover::new(
        principal(PrincipalKind::Human, "alice", "example.org"),
        ApprovalAssurance::PersonAuthenticated,
    )
    .expect("authenticated human");
    AttestedDecision::new(
        approver,
        DecisionIngress::new("console").expect("ingress"),
        reference("event-1"),
        7,
        approver_id(),
        Sha256Digest::of(b"the rendering"),
        approve,
    )
    .expect("attested decision")
}

#[test]
fn subject_digest_is_computed_never_taken() {
    let rendering = rendering("Release build 7 to the stable channel.");
    let computed = rendering.subject_digest();
    assert_eq!(computed, Sha256Digest::of(rendering.as_str().as_bytes()));
    assert!(computed.as_str().starts_with("sha256:"));

    let sealed = DecisionDescriptor::seal(hold_request(rendering.clone(), None), 10)
        .expect("seal without a given digest");
    assert_eq!(sealed.subject_digest, computed);

    let sealed =
        DecisionDescriptor::seal(hold_request(rendering.clone(), Some(computed.clone())), 10)
            .expect("seal with the same digest");
    assert_eq!(sealed.subject_digest, computed);

    let foreign = Sha256Digest::of(b"another rendering");
    assert_eq!(
        DecisionDescriptor::seal(hold_request(rendering, Some(foreign)), 10)
            .expect_err("a digest other than the rendering's is refused, never taken"),
        DecisionContractError::SubjectDigestMismatch
    );
}

#[test]
fn canonical_parts_tile_on_char_boundaries_deterministically() {
    // Multi-byte characters of every width straddle the part boundaries.
    let text = "a\u{f1}b\u{20ac}c\u{1f600}d\u{e9}".repeat(7);
    for size in [1_u32, 2, 3, 4, 5, 7, 64] {
        let max_part_bytes = NonZeroU32::new(size);
        let ranges = part_ranges(&text, max_part_bytes);
        let mut next = 0;
        for range in &ranges {
            assert_eq!(range.start, next, "parts are consecutive");
            assert!(range.end > range.start, "no part is empty");
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
            let part = text
                .get(range.clone())
                .expect("a part is a char-boundary slice");
            if part.len() > usize::try_from(size).expect("part size") {
                assert_eq!(
                    part.chars().count(),
                    1,
                    "only one wide character exceeds the size"
                );
            }
            next = range.end;
        }
        assert_eq!(next, text.len(), "the parts tile the whole rendering");

        let parts = canonical_parts(&text, max_part_bytes);
        assert_eq!(
            parts,
            canonical_parts(&text, max_part_bytes),
            "deterministic"
        );
        assert_eq!(parts.len(), ranges.len());
        for (position, (part, range)) in parts.iter().zip(&ranges).enumerate() {
            assert_eq!(usize::try_from(part.index).expect("index"), position);
            assert_eq!(usize::try_from(part.count).expect("count"), ranges.len());
            let bytes = text.get(range.clone()).expect("part").as_bytes();
            assert_eq!(part.part_digest, Sha256Digest::of(bytes));
        }
    }
    assert_eq!(
        canonical_parts(&text, None).len(),
        1,
        "no part size is one part"
    );
    assert_eq!(
        canonical_parts(&text, NonZeroU32::new(u32::MAX)).len(),
        1,
        "a rendering that fits one part is one part"
    );
}

#[test]
fn a_one_part_and_a_multi_part_presentation_are_complete() {
    let rendering = rendering(&"Release build 7 \u{20ac} stable channel. ".repeat(4));
    let approver = PresentationAudience::Approver {
        approver_id: approver_id(),
    };

    let one = PresentationRecord::complete(
        approver.clone(),
        &rendering,
        None,
        reference("console"),
        reference("message-1"),
        5,
    );
    assert_eq!(one.parts_shown.len(), 1);
    assert_eq!(one.subject_digest, rendering.subject_digest());
    assert!(one.shows_every_part_of(&rendering));

    let multi = PresentationRecord::complete(
        approver,
        &rendering,
        NonZeroU32::new(16),
        reference("channel-1"),
        reference("message-2"),
        6,
    );
    assert!(multi.parts_shown.len() > 1);
    assert!(multi.shows_every_part_of(&rendering));
}

#[test]
fn a_presentation_missing_a_part_is_incomplete() {
    let rendering = rendering(&"Release build 7 to the stable channel. ".repeat(4));
    let complete = PresentationRecord::complete(
        PresentationAudience::Approver {
            approver_id: approver_id(),
        },
        &rendering,
        NonZeroU32::new(16),
        reference("channel-1"),
        reference("message-1"),
        5,
    );
    assert!(complete.parts_shown.len() > 2);

    let mut missing = complete.clone();
    missing.parts_shown.remove(1);
    assert!(!missing.shows_every_part_of(&rendering));

    let mut last_missing = complete.clone();
    last_missing.parts_shown.pop();
    assert!(!last_missing.shows_every_part_of(&rendering));

    // Parts split for one size are judged against the split for the
    // record's own size.
    let mut other_size = complete.clone();
    other_size.max_part_bytes = NonZeroU32::new(17);
    assert!(!other_size.shows_every_part_of(&rendering));

    assert!(!complete.shows_every_part_of(&self::rendering("another rendering")));
}

#[test]
fn decision_id_derives_from_kind_and_subject_key() {
    let kind = DecisionKind::new("identity.create").expect("kind");
    let subject = DecisionSubjectKey::new("mob-1/helper").expect("subject key");
    let id = DecisionId::derive(&kind, &subject);
    let expected = format!(
        "decision:{:x}",
        Sha256::digest(br#"["identity.create","mob-1/helper"]"#)
    );
    assert_eq!(id.as_str(), expected);
    assert_eq!(
        DecisionId::new(id.as_str()).expect("a derived id parses"),
        id
    );
    assert_eq!(id, DecisionId::derive(&kind, &subject), "stable");

    assert_ne!(
        id,
        DecisionId::derive(&DecisionKind::new("release").expect("kind"), &subject)
    );
    assert_ne!(
        id,
        DecisionId::derive(
            &kind,
            &DecisionSubjectKey::new("mob-1/other").expect("subject key")
        )
    );
    // The array encoding keeps a separator inside a value from colliding.
    assert_ne!(
        DecisionId::derive(
            &DecisionKind::new("a.b").expect("kind"),
            &DecisionSubjectKey::new("c").expect("subject key")
        ),
        DecisionId::derive(
            &DecisionKind::new("a").expect("kind"),
            &DecisionSubjectKey::new("b.c").expect("subject key")
        )
    );
    for invalid in ["decision:", "decision:XYZ", "sha256:00", &"0".repeat(64)] {
        assert!(DecisionId::new(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn evidence_digest_changes_with_any_evidence_field() {
    let human = attested(true)
        .with_presentation(PresentationRecord::complete(
            PresentationAudience::Approver {
                approver_id: approver_id(),
            },
            &rendering("the rendering"),
            None,
            reference("console"),
            reference("message-1"),
            5,
        ))
        .with_reason("looks right")
        .expect("reason")
        .evidence();
    let base = human.digest().expect("digest");
    assert_eq!(base, human.digest().expect("digest"), "stable");
    assert_eq!(human.tier(), DecisionTier::Human);
    assert!(
        matches!(
            &human,
            DecisionEvidence::Human {
                observed_at_ms: Some(7),
                ..
            }
        ),
        "the evidence records when the ingress observed the decision"
    );

    let changed = |change: &dyn Fn(&mut DecisionEvidence)| {
        let mut evidence = human.clone();
        change(&mut evidence);
        assert_ne!(evidence, human, "the change must change the evidence");
        evidence.digest().expect("digest")
    };
    let changes: [(&str, &dyn Fn(&mut DecisionEvidence)); 13] = [
        ("decider", &|evidence| {
            if let DecisionEvidence::Human { decider, .. } = evidence {
                *decider = key(PrincipalKind::Human, "bob");
            }
        }),
        ("principal", &|evidence| {
            if let DecisionEvidence::Human { principal: p, .. } = evidence {
                *p = principal(PrincipalKind::Human, "alice", "other.example");
            }
        }),
        ("assurance", &|evidence| {
            if let DecisionEvidence::Human { assurance, .. } = evidence {
                *assurance = ApprovalAssurance::ServiceCredential;
            }
        }),
        ("authentication scope", &|evidence| {
            if let DecisionEvidence::Human {
                authentication_scope,
                ..
            } = evidence
            {
                *authentication_scope = Some("login-2".to_owned());
            }
        }),
        ("ingress", &|evidence| {
            if let DecisionEvidence::Human { ingress, .. } = evidence {
                *ingress = DecisionIngress::new("chat").expect("ingress");
            }
        }),
        ("event", &|evidence| {
            if let DecisionEvidence::Human { event_ref, .. } = evidence {
                *event_ref = reference("event-2");
            }
        }),
        ("observed time", &|evidence| {
            if let DecisionEvidence::Human { observed_at_ms, .. } = evidence {
                *observed_at_ms = Some(8);
            }
        }),
        ("no observed time", &|evidence| {
            if let DecisionEvidence::Human { observed_at_ms, .. } = evidence {
                *observed_at_ms = None;
            }
        }),
        ("approver id", &|evidence| {
            if let DecisionEvidence::Human { approver_id, .. } = evidence {
                *approver_id = ApproverId::new("team:others").expect("approver id");
            }
        }),
        ("presented digest", &|evidence| {
            if let DecisionEvidence::Human {
                presented_digest, ..
            } = evidence
            {
                *presented_digest = Sha256Digest::of(b"another rendering");
            }
        }),
        ("presentation", &|evidence| {
            if let DecisionEvidence::Human { presentation, .. } = evidence {
                *presentation = None;
            }
        }),
        ("approve", &|evidence| {
            if let DecisionEvidence::Human { approve, .. } = evidence {
                *approve = false;
            }
        }),
        ("reason", &|evidence| {
            if let DecisionEvidence::Human { reason, .. } = evidence {
                *reason = Some("looks wrong".to_owned());
            }
        }),
    ];
    for (field, change) in changes {
        assert_ne!(changed(change), base, "{field} must change the digest");
    }

    let review = AttributedReview::new(
        principal(PrincipalKind::SharedAgent, "reviewer", "example.org"),
        SessionId::new(),
        reference("call-1"),
        DecisionReviewVerdict::Allow,
        Sha256Digest::of(b"the rendering"),
    )
    .expect("attributed review");
    let review_evidence = review.evidence();
    assert_eq!(review_evidence.tier(), DecisionTier::Review);
    assert_ne!(review_evidence.digest().expect("digest"), base);
    let mut other_verdict = review_evidence.clone();
    if let DecisionEvidence::Review { verdict, .. } = &mut other_verdict {
        *verdict = DecisionReviewVerdict::Deny;
    }
    assert_ne!(
        other_verdict.digest().expect("digest"),
        review_evidence.digest().expect("digest")
    );
}

#[test]
fn principal_keys_compare_qualified_principals() {
    let alice = principal(PrincipalKind::Human, "alice", "example.org");
    let key = DecisionPrincipalKey::of(&alice).expect("key");
    assert_eq!(
        key,
        DecisionPrincipalKey::of(&principal(PrincipalKind::Human, "alice", "example.org"))
            .expect("key"),
        "an equal principal has the same key"
    );
    assert_eq!(key.as_str(), r#"principal:["human","example.org","alice"]"#);
    for other in [
        principal(PrincipalKind::Human, "alice", "other.example"),
        principal(PrincipalKind::Human, "bob", "example.org"),
        principal(PrincipalKind::PersonalAgent, "alice", "example.org"),
    ] {
        assert_ne!(
            DecisionPrincipalKey::of(&other).expect("key"),
            key,
            "{other:?}"
        );
    }
    // A separator inside the domain or the id does not collide.
    assert_ne!(
        DecisionPrincipalKey::of(&principal(PrincipalKind::Human, "b", "a:")).expect("key"),
        DecisionPrincipalKey::of(&principal(PrincipalKind::Human, ":b", "a")).expect("key"),
    );
    let unqualified = PrincipalRef::new(PrincipalKind::Human, "alice").expect("principal");
    assert_eq!(
        DecisionPrincipalKey::of(&unqualified).expect_err("an unqualified principal has no key"),
        DecisionContractError::UnqualifiedPrincipal
    );
    // A key round trips through its text, and only the canonical text is a
    // key.
    assert_eq!(DecisionPrincipalKey::new(key.as_str()).expect("key"), key);
    for invalid in [
        r#"principal:["human", "example.org", "alice"]"#,
        r#"principal:["robot","example.org","alice"]"#,
        r#"principal:["human","","alice"]"#,
        r#"["human","example.org","alice"]"#,
    ] {
        assert!(DecisionPrincipalKey::new(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn kind_tokens_are_validated() {
    let longest = "k".repeat(DECISION_TOKEN_MAX_BYTES);
    for valid in [
        "release",
        "identity.create",
        "a",
        "x-1_2.3",
        longest.as_str(),
    ] {
        assert_eq!(
            DecisionKind::new(valid).expect("valid kind").as_str(),
            valid
        );
    }
    let too_long = "k".repeat(DECISION_TOKEN_MAX_BYTES + 1);
    for invalid in [
        "",
        "Release",
        "with space",
        "slash/kind",
        "\u{fc}mlaut",
        too_long.as_str(),
    ] {
        assert_eq!(
            DecisionKind::new(invalid).expect_err("invalid kind"),
            DecisionContractError::Invalid {
                what: "decision kind"
            },
            "{invalid:?}"
        );
    }
    // A stored value is validated as it decodes.
    assert!(serde_json::from_str::<DecisionKind>(r#""Release""#).is_err());
    assert_eq!(
        serde_json::from_str::<DecisionKind>(r#""release""#)
            .expect("kind")
            .as_str(),
        "release"
    );
}

/// A sealed row of `subject` in `phase`, owing `notice`.
fn stored(
    subject: &str,
    phase: DecisionPhase,
    notice: DecisionNotice,
    deadline: u64,
) -> StoredDecision {
    let mut request = hold_request(rendering("Release build 7."), None);
    request.subject_key = DecisionSubjectKey::new(subject).expect("subject key");
    request.deadline_at_ms = deadline;
    let descriptor = DecisionDescriptor::seal(request, 10).expect("seal");
    let decision_id = descriptor.decision_id();
    let state = DecisionMachineState {
        phase,
        decision_id: decision_id.as_str().to_owned(),
        deadline_at_ms: deadline,
        subject_digest: descriptor.subject_digest.as_str().to_owned(),
        operation_id: descriptor.operation_id.as_str().to_owned(),
        request_digest: descriptor.request_digest.as_str().to_owned(),
        requester: descriptor.requester.as_str().to_owned(),
        approver_id: format!("approver:{}", descriptor.approver_id),
        reviewer: None,
        stage: DecisionStage::Human,
        first_presented_to_approver_at_ms: None,
        presented_to_reviewer: false,
        evidence_digest: None,
        decided_at_ms: None,
        ended_at_ms: None,
        permit_issued: false,
        executed_request_digest: None,
        notice,
    };
    StoredDecision::held(descriptor, state)
}

#[test]
fn the_in_memory_store_writes_conditionally_and_pages_its_indexes() {
    let store = InMemoryDecisionStore::new();
    let open = stored(
        "subject-open",
        DecisionPhase::Open,
        DecisionNotice::NotDue,
        50,
    );
    let approved = stored(
        "subject-approved",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        500,
    );
    let owed = stored(
        "subject-owed",
        DecisionPhase::Rejected,
        DecisionNotice::Owed,
        50,
    );
    let mut held = stored(
        "subject-held",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        50,
    );
    held.state.permit_issued = true;
    for row in [&open, &approved, &owed, &held] {
        store.insert_if_absent(row).expect("insert");
    }
    assert_eq!(
        store.insert_if_absent(&open),
        Err(ApprovalStoreError::Conflict),
        "one row per decision"
    );

    let mut next = open.clone();
    next.revision = 2;
    assert_eq!(
        store.replace_if_revision(2, &next),
        Err(ApprovalStoreError::Conflict),
        "a stale revision writes nothing"
    );
    store.replace_if_revision(1, &next).expect("replace");
    assert_eq!(
        store.get(&open.decision_id).expect("get"),
        Some(DecisionRow::Readable(Box::new(next)))
    );
    let missing = stored(
        "subject-missing",
        DecisionPhase::Open,
        DecisionNotice::NotDue,
        50,
    );
    assert_eq!(
        store.replace_if_revision(1, &missing),
        Err(ApprovalStoreError::Conflict),
        "no row to replace"
    );

    let ids = |page: DecisionPage| -> Vec<String> {
        page.rows
            .iter()
            .map(|row| row.decision_id().to_owned())
            .collect()
    };
    let mut owed_ids = vec![
        approved.decision_id.as_str().to_owned(),
        owed.decision_id.as_str().to_owned(),
        held.decision_id.as_str().to_owned(),
    ];
    owed_ids.sort();
    assert_eq!(ids(store.owed(None, 10).expect("owed")), owed_ids);
    assert_eq!(
        ids(store.due(100, None, 10).expect("due")),
        vec![open.decision_id.as_str().to_owned()],
        "the approved row is before its deadline, and the held one waits for \
         its permit's settlement"
    );
    let filter = DecisionListFilter {
        phase: Some(DecisionPhase::Open),
        ..DecisionListFilter::default()
    };
    assert_eq!(store.count(&filter).expect("count"), 1);
    assert_eq!(
        store.count(&DecisionListFilter::default()).expect("count"),
        4
    );

    let first = store
        .list(&DecisionListFilter::default(), None, 2)
        .expect("first page");
    assert_eq!(first.rows.len(), 2);
    let after = first.next_after.expect("a next page");
    let second = store
        .list(&DecisionListFilter::default(), Some(&after), 2)
        .expect("second page");
    assert_eq!(second.rows.len(), 2);
    assert_eq!(second.next_after, None);
    assert!(!store.is_persistent());
}

#[test]
fn an_approved_decision_with_no_permit_past_its_deadline_is_due() {
    let store = InMemoryDecisionStore::new();
    let lapsed = stored(
        "subject-lapsed",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        50,
    );
    let mut held = stored(
        "subject-held",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        50,
    );
    held.state.permit_issued = true;
    let fresh = stored(
        "subject-fresh",
        DecisionPhase::Approved,
        DecisionNotice::NotDue,
        500,
    );
    for row in [&lapsed, &held, &fresh] {
        store.insert_if_absent(row).expect("insert");
    }
    let due = |now_ms: u64| -> Vec<String> {
        store
            .due(now_ms, None, 10)
            .expect("due")
            .rows
            .iter()
            .map(|row| row.decision_id().to_owned())
            .collect()
    };
    let lapsed_only = vec![lapsed.decision_id.as_str().to_owned()];
    assert!(due(49).is_empty(), "before its deadline nothing is due");
    assert_eq!(due(50), lapsed_only, "at its deadline it is due");
    assert_eq!(
        due(100),
        lapsed_only,
        "past it, while the held one waits for its permit's settlement"
    );
    assert!(decision_is_due(&lapsed, 50));
    assert!(!decision_is_due(&lapsed, 49));
    assert!(!decision_is_due(&held, 100));
    assert!(!decision_is_due(&fresh, 100));
}

/// The pinned JSON of the human evidence of
/// `the_evidence_encoding_and_its_digest_are_pinned`.
const HUMAN_EVIDENCE_JSON: &str = concat!(
    r#"{"kind":"human","#,
    r#""decider":"principal:[\"human\",\"example.org\",\"alice\"]","#,
    r#""principal":{"kind":"human","id":"alice","#,
    r#""qualification":{"kind":"qualified","trust_domain_id":"example.org"}},"#,
    r#""assurance":"person_authenticated","#,
    r#""authentication_scope":"login-1","#,
    r#""ingress":"web","#,
    r#""event_ref":"event-1","#,
    r#""approver_id":"team:owners","#,
    r#""presented_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b","#,
    r#""presentation":{"#,
    r#""audience":{"kind":"approver","approver_id":"team:owners"},"#,
    r#""subject_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b","#,
    r#""parts_shown":[{"index":0,"count":1,"#,
    r#""part_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b"}],"#,
    r#""channel_ref":"channel-1","#,
    r#""message_ref":"message-1","#,
    r#""at_ms":5},"#,
    r#""approve":true,"#,
    r#""reason":"looks right"}"#,
);
const HUMAN_EVIDENCE_DIGEST: &str =
    "sha256:376dacd6e9e2f1b4ecc7d0b37aefbea51499a23c927c2533dc662920bf36757f";

/// The pinned JSON of the review evidence of
/// `the_evidence_encoding_and_its_digest_are_pinned`.
const REVIEW_EVIDENCE_JSON: &str = concat!(
    r#"{"kind":"review","#,
    r#""reviewer":"principal:[\"shared_agent\",\"example.org\",\"reviewer\"]","#,
    r#""principal":{"kind":"shared_agent","id":"reviewer","#,
    r#""qualification":{"kind":"qualified","trust_domain_id":"example.org"}},"#,
    r#""origin_session":"018f2c3a-6b7d-7e8f-9a0b-1c2d3e4f5a6b","#,
    r#""verdict_call_ref":"call-1","#,
    r#""verdict":"allow","#,
    r#""presented_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b","#,
    r#""feedback":"matches the request"}"#,
);
const REVIEW_EVIDENCE_DIGEST: &str =
    "sha256:6029e98bbc45c98c4cfca425736730f8b1a6008fdc44287948dd36c033e8c44b";

/// The evidence a decision binds by digest has a pinned encoding: a change
/// to how the evidence, or any type it embeds (a principal, an assurance, a
/// session id, a presentation record, a canonical part), serializes changes
/// these bytes and fails here, before it would tear every decided row. Such
/// a change needs a new evidence domain and a migration of the stored rows.
#[test]
fn the_evidence_encoding_and_its_digest_are_pinned() {
    let rendering = rendering("Release build 7.");
    let human = DecisionEvidence::Human {
        decider: key(PrincipalKind::Human, "alice"),
        principal: principal(PrincipalKind::Human, "alice", "example.org"),
        assurance: ApprovalAssurance::PersonAuthenticated,
        authentication_scope: Some("login-1".to_owned()),
        ingress: DecisionIngress::new("web").expect("ingress"),
        event_ref: reference("event-1"),
        observed_at_ms: None,
        approver_id: approver_id(),
        presented_digest: rendering.subject_digest(),
        presentation: Some(PresentationRecord::complete(
            PresentationAudience::Approver {
                approver_id: approver_id(),
            },
            &rendering,
            None,
            reference("channel-1"),
            reference("message-1"),
            5,
        )),
        approve: true,
        reason: Some("looks right".to_owned()),
    };
    let mut human_observed = human.clone();
    if let DecisionEvidence::Human { observed_at_ms, .. } = &mut human_observed {
        *observed_at_ms = Some(7);
    }
    let review = DecisionEvidence::Review {
        reviewer: key(PrincipalKind::SharedAgent, "reviewer"),
        principal: principal(PrincipalKind::SharedAgent, "reviewer", "example.org"),
        origin_session: SessionId::parse("018f2c3a-6b7d-7e8f-9a0b-1c2d3e4f5a6b")
            .expect("session id"),
        verdict_call_ref: reference("call-1"),
        verdict: DecisionReviewVerdict::Allow,
        presented_digest: rendering.subject_digest(),
        feedback: Some("matches the request".to_owned()),
    };
    for (evidence, json, digest) in [
        (&human, HUMAN_EVIDENCE_JSON, HUMAN_EVIDENCE_DIGEST),
        (
            &human_observed,
            HUMAN_OBSERVED_EVIDENCE_JSON,
            HUMAN_OBSERVED_EVIDENCE_DIGEST,
        ),
        (&review, REVIEW_EVIDENCE_JSON, REVIEW_EVIDENCE_DIGEST),
    ] {
        let tier = evidence.tier();
        assert_eq!(
            serde_json::to_string(evidence).expect("encode"),
            json,
            "the {tier:?} evidence's bytes"
        );
        assert_eq!(
            evidence.digest().expect("digest").as_str(),
            digest,
            "the {tier:?} evidence's digest"
        );
        assert_eq!(
            &serde_json::from_str::<DecisionEvidence>(json).expect("the pinned bytes decode"),
            evidence,
            "the {tier:?} evidence decodes from its pinned bytes"
        );
    }
}

/// The pinned JSON of the human evidence of
/// `the_evidence_encoding_and_its_digest_are_pinned`, recorded with the time
/// the decision event's ingress observed it (7).
const HUMAN_OBSERVED_EVIDENCE_JSON: &str = concat!(
    r#"{"kind":"human","#,
    r#""decider":"principal:[\"human\",\"example.org\",\"alice\"]","#,
    r#""principal":{"kind":"human","id":"alice","#,
    r#""qualification":{"kind":"qualified","trust_domain_id":"example.org"}},"#,
    r#""assurance":"person_authenticated","#,
    r#""authentication_scope":"login-1","#,
    r#""ingress":"web","#,
    r#""event_ref":"event-1","#,
    r#""observed_at_ms":7,"#,
    r#""approver_id":"team:owners","#,
    r#""presented_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b","#,
    r#""presentation":{"#,
    r#""audience":{"kind":"approver","approver_id":"team:owners"},"#,
    r#""subject_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b","#,
    r#""parts_shown":[{"index":0,"count":1,"#,
    r#""part_digest":"#,
    r#""sha256:6d7131a1a32893cc4a947ca6d83471bca641ab725a2ef83dd356f59c0b4e3f5b"}],"#,
    r#""channel_ref":"channel-1","#,
    r#""message_ref":"message-1","#,
    r#""at_ms":5},"#,
    r#""approve":true,"#,
    r#""reason":"looks right"}"#,
);
const HUMAN_OBSERVED_EVIDENCE_DIGEST: &str =
    "sha256:17355bf023b24670e47d692b6103f4456397ad2aa8d561921a4dd8c84c0020b2";

/// A human decision's evidence records when its ingress observed the
/// decision event, under the same evidence domain: its pinned bytes decode
/// and encode back unchanged. Evidence recorded before it carried that time
/// decodes without it and keeps its bytes and its digest, so a decided row
/// written then stays readable; being decided, it needs no ordering check.
#[test]
fn the_evidence_records_the_observed_time_and_older_evidence_keeps_its_digest() {
    for (json, digest) in [
        (HUMAN_OBSERVED_EVIDENCE_JSON, HUMAN_OBSERVED_EVIDENCE_DIGEST),
        (HUMAN_EVIDENCE_JSON, HUMAN_EVIDENCE_DIGEST),
    ] {
        let evidence: DecisionEvidence =
            serde_json::from_str(json).expect("the pinned bytes decode");
        assert_eq!(
            serde_json::to_string(&evidence).expect("encode"),
            json,
            "the evidence encodes back to its pinned bytes"
        );
        assert_eq!(
            evidence.digest().expect("digest").as_str(),
            digest,
            "the evidence keeps its pinned digest"
        );
    }
}
