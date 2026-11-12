//! Rows of the interface every part shares: the closed vocabularies, the
//! operation id, the typed outcome, the normalization of an archive, an
//! unarchive and an adoption, the name guard, and what the executor seam
//! refuses before Core is called.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::*;
use crate::identity_first::{AgentAddressability, AgentIdentity, DisplayName, DurableAgentSpec};

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
    // A name in the reserved member-id namespace, which every public door
    // refuses, and a name in the namespace of generated runtime aliases
    // (every name that starts with `rt:`, with or without a generation),
    // whose public alias would be another agent's: each is an agent name
    // in form, and none is admitted.
    const RESERVED: [&str; 4] = ["mk--keeper", "rt:keeper:3", "rt:keeper", "rt:a:b"];
    for name in RESERVED {
        assert!(AgentIdentity::parse(name).is_ok(), "{name}");
        assert_eq!(core_member_name(name), None, "{name}");
    }

    let requesters = [
        RequesterFacts::agent("helper", "worker", BTreeMap::new()),
        RequesterFacts::host("ops"),
        RequesterFacts::human("person-1"),
    ];
    // An adoption of an agent of such a name is refused the same way: a
    // configured agent of a reserved name is not released from
    // configuration under it.
    let adopting = policy(allow(&["adopt"]));
    for requester in &requesters {
        for name in ["rt:keeper", "mk--keeper"] {
            let refused = normalize_adopt(
                &adopting,
                requester,
                RequestEnvelope::default(),
                &entry(name),
                &AdoptIdentitySpec::new(name, digest(name)),
            )
            .expect_err("the name is refused");
            assert_eq!(
                refused.to_data(),
                json!({"kind": "invalid_name", "field": "name"}),
                "{name}: {:?}",
                requester.requester
            );
        }
    }
    for requester in &requesters {
        for name in RESERVED {
            let refused = create_of(requester, name).expect_err("the name is refused");
            assert_eq!(
                refused.to_data(),
                json!({"kind": "invalid_name", "field": "agents[0].name"}),
                "{name}: {:?}",
                requester.requester
            );
        }
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

    let origin = ToolCallOrigin::new("mob-1", "session-1", "run-1", "round-1", "call-1");
    let derived = OperationId::for_tool_call(&agent, &origin);
    assert_eq!(derived, OperationId::for_tool_call(&agent, &origin));
    assert_ne!(
        derived,
        OperationId::for_tool_call(
            &agent,
            &ToolCallOrigin::new("mob-1", "session-1", "run-2", "round-1", "call-1")
        ),
        "the same tool call id in another run is another operation"
    );
    assert_ne!(
        derived,
        OperationId::for_tool_call(
            &agent,
            &ToolCallOrigin::new("mob-1", "session-1", "run-1", "round-2", "call-1")
        ),
        "a model numbers its calls anew in each answer: the same call id in another round of \
         the same run is another operation"
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
        IdentityRefusal::new(IdentityRefusalKind::OwnerReadOnly).at("name"),
    )
    .to_json();
    assert_eq!(refused["outcome"], "refused");
    assert_eq!(refused["kind"], "owner_read_only");
    assert_eq!(refused["field"], "name");
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
        json!({"kind": "name_taken", "by": "identity", "field": "agents[1].name"})
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
        // Core's own words for a malformed request say where a record
        // lives; none of them is written.
        Core::InvalidRequest {
            problem: "'keeper' holds no store-owned spec to check the expected digest against: \
                      its host's roster checks its spec"
                .to_owned(),
        },
        Core::InvalidRequest {
            problem: "its host's roster holds its spec".to_owned(),
        },
        Core::InvalidRequest {
            problem: "apply the roster first".to_owned(),
        },
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
    assert_eq!(
        pending.to_data(),
        json!({"kind": "admission_unsettled", "correlation": "member_step"}),
        "Core's sentence for the step is not written"
    );
    assert!(
        refusal_of_error(meerkat_mob::MobError::Internal("store".to_owned())).is_err(),
        "an error that is no refusal stays an error"
    );
}

/// The rendering of an archive, an unarchive and an adoption names the
/// action and the identity, and parses to a structured card. No rendering
/// shows the spec digest the requester read, an update's and a removal's
/// included: a target that moved is refused `stale_spec` at the commit,
/// and the request digest still tells two expected digests apart.
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
            &AdoptIdentitySpec::new("keeper", expected),
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
            !text.contains(labels::EXPECTED_SPEC_DIGEST),
            "{action}: the rendering shows no spec digest: {text}"
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

/// The one wire spelling of the family: a full create request and an update
/// request read and write as these, field for field; an unknown field is
/// refused; and a refusal names its field as a path into the request as
/// written.
#[test]
fn the_wire_spelling_of_a_create_an_update_and_a_refusal() {
    let create = json!({
        "request_id": "req-7",
        "agents": [
            {
                "name": "keeper",
                "profile": "worker",
                "display_name": "The keeper",
                "instructions": ["Keep the records.", "Answer in one paragraph."],
                "context": {"desk": "archive"},
                "labels": {"team": "records"},
                "first_message": "Introduce yourself to the lead.",
                "peers": ["helper", "warden"]
            },
            {"name": "warden", "profile": "worker"}
        ]
    });
    let parsed: CreateAgents = serde_json::from_value(create.clone()).expect("a create request");
    assert_eq!(
        serde_json::to_value(&parsed).expect("a create request"),
        create,
        "a create request writes as it reads"
    );
    let specs = parsed
        .to_create_specs()
        .expect("the agents are well formed");
    let keeper = &specs[0];
    assert_eq!(
        (
            keeper.spec.identity.as_str(),
            keeper.spec.profile.as_str(),
            keeper.spec.display_name.as_ref().map(DisplayName::as_str),
            keeper.spec.additional_instructions.as_slice(),
            keeper.spec.context.clone(),
            keeper.spec.labels.get("team").map(String::as_str),
            keeper.spec.initial_message.is_some(),
            keeper.peers.as_slice(),
        ),
        (
            "keeper",
            "worker",
            Some("The keeper"),
            [
                "Keep the records.".to_owned(),
                "Answer in one paragraph.".to_owned()
            ]
            .as_slice(),
            Some(json!({"desk": "archive"})),
            Some("records"),
            true,
            ["helper".to_owned(), "warden".to_owned()].as_slice(),
        )
    );
    assert_eq!(
        (
            keeper.spec.runtime_mode_override,
            keeper.spec.backend,
            keeper.spec.binding.is_some(),
            keeper.spec.placement.is_some(),
        ),
        (None, None, false, false),
        "the request sets nothing it does not name"
    );
    for (unknown, what) in [
        (
            json!({"agents": [{"name": "k", "profile": "worker", "spec": {}}]}),
            "spec",
        ),
        (
            json!({"agents": [{"name": "k", "profile": "worker", "grants": []}]}),
            "grants",
        ),
        (
            json!({"agents": [{"name": "k", "profile": "worker", "schedules": []}]}),
            "schedules",
        ),
        (
            json!({"agents": [{"name": "k", "profile": "worker", "fork_from": {}}]}),
            "fork_from",
        ),
        (
            json!({"agents": [{"name": "k", "profile": "worker", "placement": "h"}]}),
            "placement",
        ),
        (
            json!({"agents": [{"identity": "k", "profile": "worker"}]}),
            "identity",
        ),
        (json!({"mob_id": "m", "agents": []}), "mob_id"),
        (json!({"identities": []}), "identities"),
    ] {
        assert!(
            serde_json::from_value::<CreateAgents>(unknown).is_err(),
            "a create request with `{what}` is refused"
        );
    }

    let expected = digest("keeper");
    let update = json!({
        "request_id": "req-8",
        "name": "keeper",
        "expected_spec_digest": expected.as_str(),
        "set": {
            "display_name": "Keeper of records",
            "instructions": ["Keep them in order."],
            "context": {"desk": "vault"},
            "labels": {"team": "vault"},
            "labels_remove": ["shift"],
            "profile": "worker"
        },
        "peers_add": ["warden"],
        "peers_remove": ["helper"]
    });
    let parsed: UpdateAgent = serde_json::from_value(update.clone()).expect("an update request");
    assert_eq!(
        serde_json::to_value(&parsed).expect("an update request"),
        update,
        "an update request writes as it reads"
    );
    let current = BTreeMap::from([
        ("team".to_owned(), "records".to_owned()),
        ("shift".to_owned(), "night".to_owned()),
    ]);
    let spec = parsed
        .to_update_spec(&policy(allow(&["descriptive"])), &current)
        .expect("the update is well formed");
    assert_eq!(
        (
            spec.identity.as_str(),
            &spec.expected_spec_digest,
            spec.set.display_name.as_ref().map(DisplayName::as_str),
            spec.set.additional_instructions.clone(),
            spec.set.descriptive_labels.clone(),
            spec.set.authority_fields.clone(),
            spec.set.peers_add.as_slice(),
            spec.set.peers_remove.as_slice(),
        ),
        (
            "keeper",
            &expected,
            Some("Keeper of records"),
            Some(vec!["Keep them in order.".to_owned()]),
            Some(BTreeMap::from([("team".to_owned(), "vault".to_owned())])),
            None,
            ["warden".to_owned()].as_slice(),
            ["helper".to_owned()].as_slice(),
        ),
        "the labels set and removed are applied to the agent's labels"
    );
    for (unknown, what) in [
        (
            json!({"identity": "keeper", "expected_spec_digest": expected.as_str()}),
            "identity",
        ),
        (
            json!({"name": "keeper", "expected_spec_digest": expected.as_str(),
                   "set": {"additional_instructions": ["x"]}}),
            "set.additional_instructions",
        ),
        (
            json!({"name": "keeper", "expected_spec_digest": expected.as_str(),
                   "set": {"peers_add": ["warden"]}}),
            "set.peers_add",
        ),
        (
            json!({"name": "keeper", "expected_spec_digest": expected.as_str(), "grants_add": []}),
            "grants_add",
        ),
    ] {
        assert!(
            serde_json::from_value::<UpdateAgent>(unknown).is_err(),
            "an update request with `{what}` is refused"
        );
    }
    let whole: AgentRef = serde_json::from_value(json!({
        "request_id": "req-9",
        "name": "keeper",
        "expected_spec_digest": expected.as_str()
    }))
    .expect("a change of one agent as a whole");
    assert_eq!(whole.to_archive().identity, "keeper");
    assert_eq!(whole.to_remove().expected_spec_digest, expected);

    let host = RequesterFacts::host("ops");
    let profiles = BTreeSet::from(["worker".to_owned()]);
    let peers = BTreeMap::from([("helper".to_owned(), PeerFacts::default())]);
    let open_peers = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "peers": {"default": "allow"},
        "approval": {"rules": allow(&["create"])},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    let normalize = |request: serde_json::Value| {
        let request: CreateAgents = serde_json::from_value(request).expect("a create request");
        let specs = request.to_create_specs()?;
        normalize_create(
            &open_peers,
            &RequestContext {
                requester: &host,
                profiles: &profiles,
                peers: &peers,
            },
            request.envelope(),
            &specs,
        )
    };
    let unknown_profile = normalize(json!({"agents": [{"name": "keeper", "profile": "ghost"}]}))
        .expect_err("the mob has no such profile");
    assert_eq!(
        unknown_profile.to_refused(),
        json!({
            "kind": "profile_unknown",
            "profile": "ghost",
            "field": "agents[0].profile",
            "retryable": false,
            "detail": "The mob has no profile of this name: name one of the mob's profiles.",
        })
    );
    // A refusal names the first offending member of the request as
    // written. Where the peer rules allow a peer by default, the first
    // offending peer is the one that is no agent the requester may name.
    let peers_request = json!({"agents": [
        {"name": "warden", "profile": "worker"},
        {"name": "keeper", "profile": "worker", "peers": ["warden", "stranger"]},
    ]});
    let unknown_peer = normalize(peers_request.clone()).expect_err("a peer nobody knows");
    assert_eq!(
        unknown_peer.to_data(),
        json!({"kind": "peer_unknown", "field": "agents[1].peers[1]"})
    );
    // Where they deny a peer by default, it is the first peer listed.
    let closed_peers = {
        let request: CreateAgents =
            serde_json::from_value(peers_request).expect("a create request");
        let specs = request
            .to_create_specs()
            .expect("the agents are well formed");
        normalize_create(
            &policy(allow(&["create"])),
            &RequestContext {
                requester: &host,
                profiles: &profiles,
                peers: &peers,
            },
            request.envelope(),
            &specs,
        )
        .expect_err("the peer rules deny by default")
    };
    assert_eq!(
        closed_peers.to_data(),
        json!({"kind": "peer_not_allowed", "identity": "warden", "field": "agents[1].peers[0]"})
    );
    let bad_name = normalize(json!({"agents": [
        {"name": "warden", "profile": "worker"},
        {"name": "not a name", "profile": "worker"},
    ]}))
    .expect_err("a name with a space");
    assert_eq!(
        bad_name.to_data(),
        json!({"kind": "invalid_name", "field": "agents[1].name"})
    );
    let no_agents = normalize(json!({"agents": []})).expect_err("nothing to create");
    assert_eq!(
        no_agents.to_data(),
        json!({"kind": "invalid_request", "problem": "empty", "field": "agents"})
    );

    let own = Requester::Host {
        surface: "ops".to_owned(),
    };
    let by_request: OperationRef =
        serde_json::from_value(json!({"request_id": "req-7"})).expect("an operation");
    assert_eq!(
        by_request.resolve(&own).expect("the caller's own request"),
        OperationId::for_request(&own, "req-7").expect("a request id")
    );
    let named = OperationId::for_request(&own, "req-7").expect("a request id");
    let by_id: OperationRef =
        serde_json::from_value(json!({"operation_id": named.as_str()})).expect("an operation");
    assert_eq!(by_id.resolve(&own).expect("the named operation"), named);
    assert!(OperationRef::default().resolve(&own).is_err());
}

/// A policy may name the change classes `archive` and `adopt` in a rule and
/// give each its decision timeout; a class the vocabulary lacks is refused.
#[test]
fn a_policy_may_name_the_archive_and_adopt_change_classes() {
    let named = IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {
            "rules": [
                {"change": ["archive"], "outcome": {"kind": "notify", "approver": {"id": "owner"}}},
                {"change": ["adopt"],
                 "outcome": {"kind": "require_approval", "approver": {"id": "owner"}}},
            ],
            "timeouts_ms": {"archive": 60_000, "adopt": 120_000},
        },
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    let host = RequesterFacts::host("ops");
    let target = entry("keeper");
    let archive = normalize_archive(
        &named,
        &host,
        RequestEnvelope::default(),
        &target,
        &ArchiveIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect("the archive is planned");
    assert_eq!(
        (
            archive.plan.kind(),
            archive.plan.change,
            archive.plan.timeout_ms
        ),
        (ApprovalKind::Notify, ChangeClass::Archive, 60_000)
    );
    let adopt = normalize_adopt(
        &named,
        &host,
        RequestEnvelope::default(),
        &target,
        &AdoptIdentitySpec::new("keeper", digest("keeper")),
    )
    .expect("the adoption is planned");
    assert_eq!(
        (adopt.plan.kind(), adopt.plan.change, adopt.plan.timeout_ms),
        (ApprovalKind::RequireApproval, ChangeClass::Adopt, 120_000)
    );
    assert!(
        IdentityCreationPolicy::parse(&json!({
            "enabled": true,
            "approval": {"rules": [{"change": ["reset"], "outcome": {"kind": "allow"}}]},
        }))
        .is_err(),
        "a change class the vocabulary lacks is refused"
    );
}

/// The closed vocabularies' token lists are complete and are what each
/// value writes: the lists the SDKs' token files are compared with.
#[test]
fn the_closed_vocabularies_list_every_wire_token() {
    assert_eq!(IdentityRefusalKind::ALL_TOKENS.len(), 57);
    assert_eq!(
        IdentityRefusalKind::ALL_TOKENS
            .iter()
            .collect::<BTreeSet<_>>()
            .len(),
        57,
        "no token twice"
    );
    for token in [
        "owner_read_only",
        "private_requester",
        "unsupported",
        "change_denied",
    ] {
        assert!(IdentityRefusalKind::ALL_TOKENS.contains(&token), "{token}");
    }
    assert_eq!(
        IdentityOutcome::ALL_TOKENS,
        ["done", "pending_decision", "pending_preparation", "refused"]
    );
    assert_eq!(
        InstructionOutcome::ALL_TOKENS,
        [
            "applies_at_next_run",
            "applies_after_compaction",
            "applies_at_next_session",
            "unchanged"
        ]
    );
    assert_eq!(
        InstructionNextSessionReason::ALL_TOKENS,
        [
            "prepared_authorization_required",
            "slot_limit_exceeded",
            "active_set_budget_exceeded",
            "lowering_unavailable",
            "body_too_large",
            "record_unavailable"
        ]
    );
    assert_eq!(AgentSeat::ALL_TOKENS, ["seated", "owed"]);
    assert_eq!(
        AgentChangeState::ALL_TOKENS,
        [
            "committed",
            "pending_preparation",
            "archiving",
            "archived",
            "restored",
            "retiring",
            "removed"
        ]
    );
    assert_eq!(
        NameTakenBy::ALL_TOKENS,
        ["identity", "removed", "worker", "unreadable"]
    );
    assert_eq!(
        NotAdoptableReason::ALL_TOKENS,
        [
            "nothing_to_adopt",
            "not_an_agent",
            "placed",
            "no_session",
            "hosted_on_first_use",
            "awaiting_held_creation",
            "awaiting_host_preparation",
            "profile_override"
        ]
    );
    assert_eq!(
        RequestProblem::ALL_TOKENS,
        ["empty", "conflicting", "malformed"]
    );

    // Four vocabularies are written by their serde form. Each variant, in
    // declaration order, writes the token its list holds at that place, and
    // the list holds no token beyond them: a variant added to one of them
    // must be listed with its token to compile, which makes its list one
    // longer than the variants here.
    fn written<T: serde::Serialize>(value: T) -> serde_json::Value {
        serde_json::to_value(value).expect("a value")
    }
    let seats = [AgentSeat::Seated, AgentSeat::Owed];
    assert_eq!(seats.len(), AgentSeat::ALL_TOKENS.len());
    for (seat, token) in seats.into_iter().zip(AgentSeat::ALL_TOKENS) {
        assert_eq!(written(seat), json!(token));
        assert_eq!(seat.token(), *token);
    }
    let states = [
        AgentChangeState::Committed,
        AgentChangeState::PendingPreparation,
        AgentChangeState::Archiving,
        AgentChangeState::Archived,
        AgentChangeState::Restored,
        AgentChangeState::Retiring,
        AgentChangeState::Removed,
    ];
    assert_eq!(states.len(), AgentChangeState::ALL_TOKENS.len());
    for (state, token) in states.into_iter().zip(AgentChangeState::ALL_TOKENS) {
        assert_eq!(written(state), json!(token));
        assert_eq!(state.token(), *token);
    }
    let reasons = [
        InstructionNextSessionReason::PreparedAuthorizationRequired,
        InstructionNextSessionReason::SlotLimitExceeded,
        InstructionNextSessionReason::ActiveSetBudgetExceeded,
        InstructionNextSessionReason::LoweringUnavailable,
        InstructionNextSessionReason::BodyTooLarge,
        InstructionNextSessionReason::RecordUnavailable,
    ];
    assert_eq!(
        reasons.len(),
        InstructionNextSessionReason::ALL_TOKENS.len()
    );
    for (reason, token) in reasons
        .into_iter()
        .zip(InstructionNextSessionReason::ALL_TOKENS)
    {
        assert_eq!(written(reason), json!(token));
        assert_eq!(reason.token(), *token);
    }
    let outcomes = [
        InstructionOutcome::AppliesAtNextRun,
        InstructionOutcome::AppliesAfterCompaction,
        InstructionOutcome::AppliesAtNextSession {
            reason: InstructionNextSessionReason::BodyTooLarge,
        },
        InstructionOutcome::Unchanged,
    ];
    assert_eq!(outcomes.len(), InstructionOutcome::ALL_TOKENS.len());
    for (outcome, token) in outcomes.into_iter().zip(InstructionOutcome::ALL_TOKENS) {
        assert_eq!(written(outcome)["kind"], json!(token));
        assert_eq!(outcome.token(), *token);
    }
}

/// A directory entry writes its verdict flat: `verdict`, with `lines` for
/// a decision and `reason` for a refusal when the host's rule gives one.
#[test]
fn a_directory_entry_writes_its_verdict_flat() {
    let page = DirectoryPage {
        profiles: vec![
            DirectoryProfile {
                profile: "worker".to_owned(),
                verdict: DirectoryVerdict::Allowed,
            },
            DirectoryProfile {
                profile: "lead".to_owned(),
                verdict: DirectoryVerdict::NotAllowed { reason: None },
            },
        ],
        peers: vec![
            DirectoryPeer {
                name: "keeper".to_owned(),
                display_name: Some("The keeper".to_owned()),
                profile: "worker".to_owned(),
                verdict: DirectoryVerdict::NeedsDecision {
                    lines: vec!["A peer of the records desk needs a decision.".to_owned()],
                },
            },
            DirectoryPeer {
                name: "warden".to_owned(),
                display_name: None,
                profile: "worker".to_owned(),
                verdict: DirectoryVerdict::NotAllowed {
                    reason: Some("Wardens take no peers.".to_owned()),
                },
            },
        ],
        next: Some("warden".to_owned()),
    };
    assert_eq!(
        serde_json::to_value(&page).expect("a directory page"),
        json!({
            "profiles": [
                {"profile": "worker", "verdict": "allowed"},
                {"profile": "lead", "verdict": "not_allowed"},
            ],
            "peers": [
                {
                    "name": "keeper",
                    "display_name": "The keeper",
                    "profile": "worker",
                    "verdict": "needs_decision",
                    "lines": ["A peer of the records desk needs a decision."],
                },
                {
                    "name": "warden",
                    "profile": "worker",
                    "verdict": "not_allowed",
                    "reason": "Wardens take no peers.",
                },
            ],
            "next": "warden",
        })
    );
}

/// No action's rendering shows the spec digest the requester read, so two
/// requests that differ in it alone render alike; their request digests,
/// which a decision's permit binds, differ.
#[test]
fn no_rendering_shows_the_expected_spec_digest() {
    let host = RequesterFacts::host("ops");
    let every = policy(allow(&["remove", "descriptive"]));
    let profiles = BTreeSet::from(["worker".to_owned()]);
    let peers = BTreeMap::new();
    let removal = |expected: &str| {
        let expected = digest(expected);
        let mut target = entry("keeper");
        target.spec_digest = expected.clone();
        normalize_remove(
            &every,
            &host,
            RequestEnvelope::default(),
            &target,
            &RemoveIdentitySpec::new("keeper", expected),
        )
        .expect("the removal is planned")
        .request
    };
    let update = |expected: &str| {
        let expected = digest(expected);
        let mut target = entry("keeper");
        target.spec_digest = expected.clone();
        let mut update = UpdateAgent::new("keeper", expected);
        update.set.instructions = Some(vec!["Keep them in order.".to_owned()]);
        let spec = update
            .to_update_spec(&every, &BTreeMap::new())
            .expect("the update is well formed");
        normalize_update(
            &every,
            &RequestContext {
                requester: &host,
                profiles: &profiles,
                peers: &peers,
            },
            RequestEnvelope::default(),
            &target,
            &spec,
        )
        .expect("the update is planned")
        .request
    };
    for (one, other) in [
        (removal("one spec"), removal("another spec")),
        (update("one spec"), update("another spec")),
    ] {
        let action = one.action.token();
        let shown = render_v1(&one, None).expect("a rendering");
        assert!(
            !shown.as_str().contains(labels::EXPECTED_SPEC_DIGEST),
            "{action}: {}",
            shown.as_str()
        );
        assert_eq!(
            shown.subject_digest(),
            render_v1(&other, None)
                .expect("a rendering")
                .subject_digest(),
            "{action}: the expected digest is no part of what is shown"
        );
        assert_ne!(
            one.request_digest().expect("a request digest"),
            other.request_digest().expect("a request digest"),
            "{action}: the request digest covers the expected digest"
        );
    }
}

/// The read types can be built outside this module: a list entry, its
/// provenance, a record, a directory entry and a plan each have their
/// constructor.
#[test]
fn the_read_types_have_constructors() {
    let mut agent = AgentSummary::new(
        "keeper",
        "worker",
        digest("keeper"),
        AgentReadiness::Dormant,
    );
    agent.provenance = Some(AgentProvenance::new(
        AgentEntered::Created,
        RequesterKind::Host,
        "ops",
        7,
        ConsentSummary::Allowed,
    ));
    let mut record = AgentRecord::new(agent.clone());
    record.instructions = vec!["Keep the records.".to_owned()];
    assert_eq!(
        serde_json::to_value(&record).expect("a record"),
        json!({
            "name": "keeper",
            "profile": "worker",
            "spec_digest": digest("keeper").as_str(),
            "archived": false,
            "readiness": "dormant",
            "provenance": {
                "entered": "created",
                "requester_kind": "host",
                "requester": "ops",
                "at_ms": 7,
                "consent": {"form": "allowed"},
            },
            "instructions": ["Keep the records."],
            "labels": {},
            "peers": [],
        })
    );
    let mut page = AgentPage::default();
    page.agents.push(agent);
    assert_eq!(page.agents.len(), 1);
    let mut directory = DirectoryPage::default();
    directory
        .profiles
        .push(DirectoryProfile::new("worker", DirectoryVerdict::Allowed));
    directory.peers.push(DirectoryPeer::new(
        "keeper",
        "worker",
        DirectoryVerdict::Allowed,
    ));
    assert_eq!(
        serde_json::to_value(&directory).expect("a directory"),
        json!({
            "profiles": [{"profile": "worker", "verdict": "allowed"}],
            "peers": [{"name": "keeper", "profile": "worker", "verdict": "allowed"}],
        })
    );
    let plan = ChangePlan::new(
        ApprovalKind::Allow,
        ChangeClass::Create,
        1,
        digest("request"),
        digest("rendering"),
        "Identity request: render-v1",
    );
    assert_eq!(plan.approver_id, None);
}
