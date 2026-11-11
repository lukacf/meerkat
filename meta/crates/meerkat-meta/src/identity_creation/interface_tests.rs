//! Rows of the interface every part shares: the closed vocabularies, the
//! operation id, the typed outcome, the normalization of an archive, an
//! unarchive and an adoption, the name guard, and what the executor seam
//! refuses before Core is called.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::*;
use crate::identity_first::{AgentAddressability, AgentIdentity, DurableAgentSpec};

fn digest(text: &str) -> Sha256Digest {
    Sha256Digest::of(text.as_bytes())
}

fn policy(rules: serde_json::Value) -> IdentityCreationPolicy {
    IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": rules},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled")
}

fn allow(changes: &[&str]) -> serde_json::Value {
    json!([{"change": changes, "outcome": {"kind": "allow"}}])
}

fn entry(identity: &str) -> IdentityEntryFacts {
    IdentityEntryFacts {
        identity: identity.to_owned(),
        spec_digest: digest(identity),
        revision: Some(1),
        profile: "worker".to_owned(),
        authority_labels: BTreeMap::new(),
    }
}

fn spec(identity: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(identity).expect("an identity name"),
        profile: "worker".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

fn create_of(requester: &RequesterFacts, name: &str) -> Result<NormalizedRequest, IdentityRefusal> {
    let profiles = BTreeSet::from(["worker".to_owned()]);
    let peers = BTreeMap::new();
    normalize_create(
        &policy(allow(&["create"])),
        &RequestContext {
            requester,
            profiles: &profiles,
            peers: &peers,
        },
        RequestEnvelope::default(),
        &[CreateIdentitySpec::new(spec(name))],
    )
}

/// The change classes and the request actions are complete for the six
/// operations: a later class or action would be a vocabulary change.
#[test]
fn the_change_classes_and_actions_cover_the_six_operations() {
    let classes: Vec<&str> = [
        ChangeClass::Create,
        ChangeClass::Descriptive,
        ChangeClass::Capability,
        ChangeClass::Peers,
        ChangeClass::Authority,
        ChangeClass::Profile,
        ChangeClass::Remove,
        ChangeClass::Archive,
        ChangeClass::Adopt,
    ]
    .into_iter()
    .map(ChangeClass::token)
    .collect();
    assert_eq!(
        classes,
        [
            "create",
            "descriptive",
            "capability",
            "peers",
            "authority",
            "profile",
            "remove",
            "archive",
            "adopt"
        ]
    );
    let parsed: Vec<ChangeClass> =
        serde_json::from_value(json!(["archive", "adopt"])).expect("a rule names both");
    assert_eq!(parsed, [ChangeClass::Archive, ChangeClass::Adopt]);

    let host = RequesterFacts::host("ops");
    let target = entry("keeper");
    let every = policy(allow(&["archive", "adopt", "remove"]));
    let archived = normalize_archive(
        &every,
        &host,
        RequestEnvelope::default(),
        &target,
        &ArchiveIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect("an archive is planned");
    let unarchived = normalize_unarchive(
        &every,
        &host,
        RequestEnvelope::default(),
        &target,
        &UnarchiveIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect("an unarchive is planned");
    let adopted = normalize_adopt(
        &every,
        &host,
        RequestEnvelope::default(),
        &target,
        &AdoptIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect("an adoption is planned");
    let facts: Vec<(&str, &str, ChangeClass)> = [&archived, &unarchived, &adopted]
        .into_iter()
        .map(|normalized| {
            (
                normalized.request.action.token(),
                normalized.request.decision_kind(),
                normalized.plan.change,
            )
        })
        .collect();
    assert_eq!(
        facts,
        [
            ("archive", "identity.archive", ChangeClass::Archive),
            ("unarchive", "identity.unarchive", ChangeClass::Archive),
            ("adopt", "identity.adopt", ChangeClass::Adopt),
        ]
    );
}

/// An archive and an unarchive are governed by the `archive` class and an
/// adoption by `adopt`: a policy that allows only removals denies each,
/// naming its class.
#[test]
fn an_archive_and_an_adoption_are_denied_under_their_own_class() {
    let only_remove = policy(allow(&["remove"]));
    let host = RequesterFacts::host("ops");
    let target = entry("keeper");
    let archive = normalize_archive(
        &only_remove,
        &host,
        RequestEnvelope::default(),
        &target,
        &ArchiveIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect_err("no rule allows an archive");
    assert_eq!(
        archive.to_data(),
        json!({"kind": "change_denied", "change": "archive"})
    );
    let unarchive = normalize_unarchive(
        &only_remove,
        &host,
        RequestEnvelope::default(),
        &target,
        &UnarchiveIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect_err("no rule allows an unarchive");
    assert_eq!(
        unarchive.to_data(),
        json!({"kind": "change_denied", "change": "archive"})
    );
    let adopt = normalize_adopt(
        &only_remove,
        &host,
        RequestEnvelope::default(),
        &target,
        &AdoptIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect_err("no rule allows an adoption");
    assert_eq!(
        adopt.to_data(),
        json!({"kind": "change_denied", "change": "adopt"})
    );
}

/// An agent never archives, unarchives or adopts its own identity, as it
/// never updates or removes it; and each names the spec it read.
#[test]
fn an_archive_an_unarchive_and_an_adoption_refuse_a_self_change_and_a_stale_spec() {
    let every = policy(allow(&["archive", "adopt"]));
    let itself = RequesterFacts::agent("keeper", "worker", BTreeMap::new());
    let other = RequesterFacts::agent("helper", "worker", BTreeMap::new());
    let target = entry("keeper");
    let current = digest("keeper");
    let stale = digest("an earlier spec");
    type Normalize = fn(
        &IdentityCreationPolicy,
        &RequesterFacts,
        &IdentityEntryFacts,
        Sha256Digest,
    ) -> Result<NormalizedRequest, IdentityRefusal>;
    let changes: [(&str, Normalize); 3] = [
        ("archive", |policy, requester, entry, digest| {
            normalize_archive(
                policy,
                requester,
                RequestEnvelope::default(),
                entry,
                &ArchiveIdentitySpec::new("keeper", digest),
            )
        }),
        ("unarchive", |policy, requester, entry, digest| {
            normalize_unarchive(
                policy,
                requester,
                RequestEnvelope::default(),
                entry,
                &UnarchiveIdentitySpec::new("keeper", digest),
            )
        }),
        ("adopt", |policy, requester, entry, digest| {
            normalize_adopt(
                policy,
                requester,
                RequestEnvelope::default(),
                entry,
                &AdoptIdentitySpec::new("keeper", digest),
            )
        }),
    ];
    for (name, normalize) in changes {
        let own = normalize(&every, &itself, &target, current.clone())
            .expect_err("an agent changes no part of itself");
        assert_eq!(
            own.kind,
            IdentityRefusalKind::SelfChangeRefused,
            "{name}: {own}"
        );
        let moved = normalize(&every, &other, &target, stale.clone())
            .expect_err("the spec moved since it was read");
        assert_eq!(
            moved.to_data(),
            json!({
                "kind": "stale_spec",
                "current_digest": current.as_str(),
                "current_revision": 1,
                "field": "expected_spec_digest",
            }),
            "{name}"
        );
        assert!(
            normalize(&every, &other, &target, current.clone()).is_ok(),
            "{name}: another agent's change of the current spec is planned"
        );
    }
}

/// A name the member-id encoding maps to another member id is refused
/// `invalid_name` in normalization, whoever asks: its doors would act on
/// another member than the one the change names.
#[test]
fn a_name_the_member_id_encoding_maps_elsewhere_is_invalid_for_every_requester() {
    assert_eq!(
        core_member_name("keeper").map(|member| member.to_string()),
        Some("keeper".to_owned())
    );
    let encoded = core_member_name("domain:calendar").expect("a name that needs the encoding");
    assert_ne!(encoded.as_str(), "domain:calendar");
    assert_eq!(identity_name_of_member(&encoded), "domain:calendar");
    assert_eq!(core_member_name("mk--keeper"), None);

    let requesters = [
        RequesterFacts::agent("helper", "worker", BTreeMap::new()),
        RequesterFacts::host("ops"),
        RequesterFacts {
            requester: Requester::Human {
                principal: "person-1".to_owned(),
            },
            profile: None,
            authority_labels: BTreeMap::new(),
        },
    ];
    for requester in &requesters {
        let refused = create_of(requester, "mk--keeper").expect_err("the name is refused");
        assert_eq!(
            refused.to_data(),
            json!({"kind": "invalid_name", "field": "identities[0].spec.identity"}),
            "{:?}",
            requester.requester
        );
        assert!(
            create_of(requester, "keeper").is_ok(),
            "{:?}: a plain name is planned",
            requester.requester
        );
    }
}

/// A caller's request id becomes Core's operation id namespaced by the
/// requester: two requesters' same request id are two operations, the same
/// requester's is one, and a tool call without one derives the same id for
/// the same call.
#[test]
fn an_operation_id_is_namespaced_by_the_requester() {
    let agent = Requester::Agent {
        identity: "helper".to_owned(),
    };
    let other_agent = Requester::Agent {
        identity: "keeper".to_owned(),
    };
    let host = Requester::Host {
        surface: "helper".to_owned(),
    };
    let human = Requester::Human {
        principal: "helper".to_owned(),
    };
    let ids: Vec<OperationId> = [&agent, &other_agent, &host, &human]
        .into_iter()
        .map(|requester| OperationId::for_request(requester, "req-1").expect("a request id"))
        .collect();
    let distinct: BTreeSet<&str> = ids.iter().map(OperationId::as_str).collect();
    assert_eq!(distinct.len(), 4, "{ids:?}");
    assert_eq!(
        OperationId::for_request(&agent, "req-1").expect("a request id"),
        ids[0],
        "the same requester and request id are one operation"
    );
    for id in &ids {
        assert!(id.as_str().ends_with(".req-1"), "{id}");
        assert!(
            meerkat_core::approval::decision::DecisionOperationId::new(id.as_str()).is_ok(),
            "{id}: a decision can hold the operation"
        );
    }
    assert!(ids[0].as_str().starts_with("agent."), "{}", ids[0]);
    assert!(ids[2].as_str().starts_with("host."), "{}", ids[2]);
    assert!(ids[3].as_str().starts_with("human."), "{}", ids[3]);

    for bad in [
        "",
        "  ",
        "line\nbreak",
        &"x".repeat(REQUEST_ID_MAX_BYTES + 1),
    ] {
        let refused = OperationId::for_request(&agent, bad).expect_err("a malformed request id");
        assert_eq!(
            refused.to_data(),
            json!({"kind": "invalid_request", "problem": "malformed", "field": "request_id"})
        );
    }

    let origin = ToolCallOrigin {
        mob_id: "mob-1",
        session_id: "session-1",
        run_id: "run-1",
        tool_call_id: "call-1",
    };
    let derived = OperationId::for_tool_call(&agent, &origin);
    assert_eq!(derived, OperationId::for_tool_call(&agent, &origin));
    assert_ne!(
        derived,
        OperationId::for_tool_call(
            &agent,
            &ToolCallOrigin {
                run_id: "run-2",
                ..origin
            }
        ),
        "the same tool call id in another run is another operation"
    );
    assert_ne!(derived, OperationId::for_tool_call(&other_agent, &origin));

    for id in ids.iter().chain([&derived]) {
        assert_eq!(
            OperationId::from_recorded(id.as_str()).as_ref(),
            Some(id),
            "a recorded id reads back as itself"
        );
    }
    for text in [
        "req-1",
        "agent.req-1",
        "other.0123456789abcdef0123456789abcdef.req-1",
        "agent.XYZ.req-1",
    ] {
        assert_eq!(OperationId::from_recorded(text), None, "{text}");
    }
}

/// The typed outcome is written the same on every surface, and a refusal
/// carries its kind, field, whether it is retryable and a sentence to act
/// on.
#[test]
fn the_typed_outcome_has_four_kinds_and_a_refusal_says_what_to_do() {
    let host = Requester::Host {
        surface: "ops".to_owned(),
    };
    let operation_id = OperationId::for_request(&host, "req-1").expect("a request id");
    let mut created = AgentOutcome::new("keeper", AgentChangeState::Committed);
    created.spec_digest = Some(digest("keeper"));
    created.spec_revision = Some(1);
    created.seat = Some(AgentSeat::Owed);
    let done = IdentityOutcome::Done {
        operation_id: operation_id.clone(),
        agents: vec![created],
    };
    assert_eq!(
        done.to_json(),
        json!({
            "outcome": "done",
            "operation_id": operation_id.as_str(),
            "agents": [{
                "name": "keeper",
                "state": "committed",
                "spec_digest": digest("keeper").as_str(),
                "spec_revision": 1,
                "seat": "owed",
            }],
        })
    );
    let updated = IdentityOutcome::Done {
        operation_id: operation_id.clone(),
        agents: vec![AgentOutcome::new("keeper", AgentChangeState::Committed)],
    };
    let outcomes: Vec<serde_json::Value> = [
        InstructionOutcome::AppliesAtNextRun,
        InstructionOutcome::AppliesAfterCompaction,
        InstructionOutcome::AppliesAtNextSession {
            reason: InstructionNextSessionReason::SlotLimitExceeded,
        },
        InstructionOutcome::Unchanged,
    ]
    .into_iter()
    .map(|outcome| {
        updated
            .clone()
            .with_instruction_outcome("keeper", outcome)
            .to_json()["agents"][0]["instructions"]
            .clone()
    })
    .collect();
    assert_eq!(
        outcomes,
        [
            json!({"kind": "applies_at_next_run"}),
            json!({"kind": "applies_after_compaction"}),
            json!({"kind": "applies_at_next_session", "reason": "slot_limit_exceeded"}),
            json!({"kind": "unchanged"}),
        ]
    );
    assert_eq!(
        InstructionsChanged::between("keeper", vec!["a".to_owned()], vec!["a".to_owned()]),
        None,
        "the same text is no change"
    );
    assert!(
        InstructionsChanged::between("keeper", vec!["a".to_owned()], vec!["b".to_owned()])
            .is_some()
    );
    let reasons: Vec<&str> = [
        InstructionNextSessionReason::PreparedAuthorizationRequired,
        InstructionNextSessionReason::SlotLimitExceeded,
        InstructionNextSessionReason::ActiveSetBudgetExceeded,
        InstructionNextSessionReason::LoweringUnavailable,
        InstructionNextSessionReason::BodyTooLarge,
        InstructionNextSessionReason::RecordUnavailable,
    ]
    .into_iter()
    .map(InstructionNextSessionReason::token)
    .collect();
    assert_eq!(
        reasons,
        [
            "prepared_authorization_required",
            "slot_limit_exceeded",
            "active_set_budget_exceeded",
            "lowering_unavailable",
            "body_too_large",
            "record_unavailable",
        ]
    );
    assert_eq!(
        IdentityOutcome::PendingDecision {
            operation_id: operation_id.clone(),
            decision_id: "decision-1".to_owned(),
            deadline_ms: 42,
        }
        .to_json(),
        json!({
            "outcome": "pending_decision",
            "operation_id": operation_id.as_str(),
            "decision_id": "decision-1",
            "deadline_ms": 42,
        })
    );
    assert_eq!(
        IdentityOutcome::PendingPreparation {
            operation_id,
            agents: Vec::new(),
        }
        .token(),
        "pending_preparation"
    );

    let refused = IdentityOutcome::Refused(
        IdentityRefusal::new(IdentityRefusalKind::OwnerReadOnly).at("identity"),
    )
    .to_json();
    assert_eq!(refused["outcome"], "refused");
    assert_eq!(refused["kind"], "owner_read_only");
    assert_eq!(refused["field"], "identity");
    assert_eq!(refused["retryable"], false);
    let detail = refused["detail"].as_str().expect("a sentence");
    assert!(
        detail.contains("configuration") && detail.contains("adopt"),
        "the detail names both ways: {detail}"
    );
    let held = IdentityOutcome::refused(IdentityRefusalKind::ChangesHeld).to_json();
    assert_eq!(held["retryable"], true);
}

/// No refusal says where an agent's record lives: not a kind, not a detail
/// of a kind, not a sentence.
#[test]
fn no_refusal_names_where_a_record_lives() {
    let takers: Vec<&str> = [
        NameTakenBy::Identity,
        NameTakenBy::Removed,
        NameTakenBy::Worker,
        NameTakenBy::Unreadable,
    ]
    .into_iter()
    .map(NameTakenBy::token)
    .collect();
    assert_eq!(takers, ["identity", "removed", "worker", "unreadable"]);
    let duplicate = {
        let profiles = BTreeSet::from(["worker".to_owned()]);
        let peers = BTreeMap::new();
        let host = RequesterFacts::host("ops");
        normalize_create(
            &policy(allow(&["create"])),
            &RequestContext {
                requester: &host,
                profiles: &profiles,
                peers: &peers,
            },
            RequestEnvelope::default(),
            &[
                CreateIdentitySpec::new(spec("keeper")),
                CreateIdentitySpec::new(spec("keeper")),
            ],
        )
        .expect_err("a name twice in one request")
    };
    assert_eq!(
        duplicate.to_data(),
        json!({"kind": "name_taken", "by": "identity", "field": "identities[1].spec.identity"})
    );

    use meerkat_mob::IdentityChangeRefusal as Core;
    let name = meerkat_mob::AgentIdentity::from("keeper");
    let core = [
        Core::OwnerReadOnly { owner: "roster" },
        Core::NotAdoptable {
            identity: name.clone(),
            reason: meerkat_mob::NotAdoptableReason::StoreOwned,
        },
        Core::NotAdoptable {
            identity: name.clone(),
            reason: meerkat_mob::NotAdoptableReason::RosterOwned,
        },
        Core::NameTaken {
            identity: name.clone(),
            by: meerkat_mob::IdentityNameHolder::Identity,
        },
        Core::IdentityUnknown {
            identity: name.clone(),
        },
        Core::NotArchived { identity: name },
    ];
    for refusal in &core {
        let mapped = refusal_of_core(refusal);
        let written = mapped.to_refused().to_string();
        for word in ["roster", "store", "custody"] {
            assert!(
                !written.contains(word),
                "{}: `{word}` in {written}",
                refusal.kind()
            );
        }
    }
}

/// Every refusal of Core's change path has its kind in the closed
/// vocabulary under Core's own token, and is retryable exactly when Core
/// says so. A member step in flight is the retryable `admission_unsettled`.
#[test]
fn a_refusal_of_cores_keeps_its_token_and_whether_it_is_retryable() {
    use meerkat_mob::IdentityChangeRefusal as Core;
    let name = meerkat_mob::AgentIdentity::from("keeper");
    let profile = meerkat_mob::ProfileName::from("worker");
    let core = vec![
        Core::InvalidRequest {
            problem: "a peer listed twice".to_owned(),
        },
        Core::BatchTooLarge { max: 256 },
        Core::ProfileUnknown {
            profile: profile.clone(),
        },
        Core::ProfileNotAllowed { profile },
        Core::PrivilegedField { field: "backend" },
        Core::GrantRevoked,
        Core::NameTaken {
            identity: name.clone(),
            by: meerkat_mob::IdentityNameHolder::Removed,
        },
        Core::OperationReused {
            operation_id: "op".to_owned(),
        },
        Core::ChangesHeld,
        Core::CandidateLaunch,
        Core::StorageUnsupported,
        Core::MobNotRunning,
        Core::SubjectWidened,
        Core::DecisionClosed { state: "expired" },
        Core::ImportRefused,
        Core::Unsupported {
            feature: "f".to_owned(),
        },
        Core::IdentityUnknown {
            identity: name.clone(),
        },
        Core::NotArchived {
            identity: name.clone(),
        },
        Core::OwnerReadOnly { owner: "roster" },
        Core::StaleSpec {
            identity: name.clone(),
            current_digest: digest("current").as_str().to_owned(),
            current_revision: Some(3),
        },
        Core::PreparationNotPending {
            identity: name.clone(),
        },
        Core::PreparationMismatch {
            identity: name.clone(),
        },
        Core::ConstructionOpen {
            identity: name.clone(),
            attempt: 1,
        },
        Core::StaleAttempt {
            identity: name.clone(),
            current: Some(2),
        },
        Core::NotConstructed {
            identity: name.clone(),
        },
        Core::ConstructionRequired {
            identity: name.clone(),
        },
        Core::ActivationPending {
            identity: name.clone(),
            attempt: 1,
        },
        Core::ConstructionRefused {
            identity: name.clone(),
            reason: "r".to_owned(),
        },
        Core::HostPreparationPending {
            identity: name.clone(),
            attempt: None,
        },
        Core::PeerUnknown {
            identity: name.clone(),
            peer: name.clone(),
        },
        Core::NotAdoptable {
            identity: name.clone(),
            reason: meerkat_mob::NotAdoptableReason::HostedOnFirstUse,
        },
        Core::NotAdoptable {
            identity: name.clone(),
            reason: meerkat_mob::NotAdoptableReason::Placed,
        },
        Core::PredecessorMismatch {
            identity: name.clone(),
        },
        Core::AdmissionUnsettled { identity: name },
        Core::DeclaredApplyOpen,
        Core::Purged,
        Core::PurgeRefused {
            reason: meerkat_mob::PurgeRefusedReason::ChangesNotHeld,
        },
    ];
    let mut tokens = BTreeSet::new();
    for refusal in &core {
        let mapped = refusal_of_core(refusal);
        assert_eq!(mapped.kind_token(), refusal.kind(), "{refusal}");
        assert_eq!(
            mapped.is_retryable(),
            refusal.is_retryable(),
            "{}: retryable",
            refusal.kind()
        );
        assert!(!mapped.detail().is_empty());
        tokens.insert(refusal.kind());
    }
    assert_eq!(
        tokens.len(),
        36,
        "every kind of Core's is mapped: {tokens:?}"
    );

    let stale = refusal_of_core(&core[19]);
    assert_eq!(
        stale.to_data(),
        json!({
            "kind": "stale_spec",
            "current_digest": digest("current").as_str(),
            "current_revision": 3,
        })
    );
    let pending = refusal_of_error(meerkat_mob::MobError::LifecycleOperationPending {
        intent: "update of keeper".to_owned(),
    })
    .expect("a member step in flight is a typed refusal");
    assert_eq!(pending.kind_token(), "admission_unsettled");
    assert!(pending.is_retryable());
    assert!(
        refusal_of_error(meerkat_mob::MobError::Internal("store".to_owned())).is_err(),
        "an error that is no refusal stays an error"
    );
}

/// The rendering of an archive, an unarchive and an adoption names the
/// action, the identity and the spec the requester read, as a removal's
/// does, and parses to a structured card.
#[test]
fn every_action_has_its_rendering() {
    let host = RequesterFacts::host("ops");
    let target = entry("keeper");
    let every = policy(allow(&["archive", "adopt"]));
    let expected = digest("keeper");
    let requests = [
        normalize_archive(
            &every,
            &host,
            RequestEnvelope::default(),
            &target,
            &ArchiveIdentitySpec::new("keeper", expected.clone()),
        ),
        normalize_unarchive(
            &every,
            &host,
            RequestEnvelope::default(),
            &target,
            &UnarchiveIdentitySpec::new("keeper", expected.clone()),
        ),
        normalize_adopt(
            &every,
            &host,
            RequestEnvelope::default(),
            &target,
            &AdoptIdentitySpec::new("keeper", expected.clone()),
        ),
    ];
    let mut digests = BTreeSet::new();
    for (normalized, action) in requests.into_iter().zip(["archive", "unarchive", "adopt"]) {
        let request = normalized.expect("the change is planned").request;
        let rendering = render_v1(&request, None).expect("a rendering");
        let text = rendering.as_str();
        assert!(
            text.contains(&format!("{}: {action}\n", labels::ACTION)),
            "{action}: {text}"
        );
        assert!(
            text.contains(&format!("{}: \"keeper\"\n", labels::IDENTITY)),
            "{action}: {text}"
        );
        assert!(
            text.contains(&format!(
                "{}: {}\n",
                labels::EXPECTED_SPEC_DIGEST,
                expected.as_str()
            )),
            "{action}: {text}"
        );
        assert!(rendering.card().is_ok(), "{action}: a structured card");
        digests.insert(rendering.subject_digest());
        digests.insert(request.request_digest().expect("a request digest"));
    }
    assert_eq!(
        digests.len(),
        6,
        "each action has its own subject digest and request digest"
    );
}

/// The policy's consent is its plan's: `allow` and `notify` commit at once,
/// and a plan that holds a decision gives none.
#[test]
fn a_plan_that_holds_a_decision_gives_no_consent() {
    let plan = |outcome| ApprovalPlan {
        outcome,
        change: ChangeClass::Create,
        timeout_ms: 1,
        raised: false,
    };
    assert!(matches!(
        consent_of_plan(&plan(PlannedOutcome::Allow)),
        Some(ChangeConsent::Policy(
            meerkat_mob::IdentityPolicyOutcome::Allow
        ))
    ));
    assert!(matches!(
        consent_of_plan(&plan(PlannedOutcome::Notify {
            approver_id: "owner".to_owned()
        })),
        Some(ChangeConsent::Policy(
            meerkat_mob::IdentityPolicyOutcome::Notify
        ))
    ));
    assert!(
        consent_of_plan(&plan(PlannedOutcome::RequireApproval {
            approver_id: "owner".to_owned()
        }))
        .is_none()
    );
}

/// The status read's record facts are additive: a status with none set is
/// written as before, and one that sets them writes each as a member of the
/// status.
#[test]
fn the_status_reads_record_facts_are_additive_members() {
    use crate::identity_first::IdentityRecordFacts;
    let unset = serde_json::to_value(IdentityRecordFacts::default()).expect("facts");
    assert_eq!(unset, json!({}));
    let set = IdentityRecordFacts {
        spec_digest: Some(digest("keeper").as_str().to_owned()),
        spec_revision: Some(2),
        archived: true,
        preparation_pending: false,
        provenance: Some(AgentProvenance {
            entered: AgentEntered::Created,
            requester_kind: RequesterKind::Agent,
            requester: "helper".to_owned(),
            at_ms: 7,
            consent: ConsentSummary::Allowed,
        }),
        open_operations: vec!["op-1".to_owned()],
    };
    let written = serde_json::to_value(&set).expect("facts");
    assert_eq!(
        written,
        json!({
            "spec_digest": digest("keeper").as_str(),
            "spec_revision": 2,
            "archived": true,
            "provenance": {
                "entered": "created",
                "requester_kind": "agent",
                "requester": "helper",
                "at_ms": 7,
                "consent": {"form": "allowed"},
            },
            "open_operations": ["op-1"],
        })
    );
    let read: IdentityRecordFacts = serde_json::from_value(written).expect("facts");
    assert_eq!(read, set);
}
