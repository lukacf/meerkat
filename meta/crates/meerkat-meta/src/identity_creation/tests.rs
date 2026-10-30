//! Rows of the identity creation policy, `render_v1`, the canonical parts
//! and the structured card. Everything here is pure: no runtime, no store.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use serde_json::{Value, json};

use super::escape::{escaped_line, is_hidden, is_markup, quoted, unescape_line, unquote};
use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn policy_json() -> Value {
    json!({
        "enabled": true,
        "reserved_keys": {"labels": ["team", "clearance"], "context": ["project"]},
        "creators": [{"id": "leads", "profile": ["lead"], "templates": ["research", "open"]}],
        "limits": {"batch": 4},
        "templates": {
            "research": {
                "title": "Research helper",
                "profiles": ["assistant", "analyst"],
                "models": "profile",
                "accounts": ["binding:notes"],
                "callbacks": [
                    {"name": "notes_read", "description": "Read notes", "input_schema": {"type": "object"}},
                    "notes_write"
                ],
                "tools": {"floor": ["notes_read"], "ceiling": ["notes_read", "notes_write", "search"]},
                "authority_fields": {"clearance": {"values": ["basic", "elevated"]}},
                "preamble": "You were created by {creator}. Peers: {peers}. You are {display_name}: {purpose}.",
                "namespace": "team",
                "max_bytes": {"display_name": 64, "purpose": 200, "instructions": 2000, "kickoff": 500}
            },
            "open": {
                "profiles": "any",
                "excess": "consequence",
                "tools": {"ceiling": ["search"]},
                "namespace": "open"
            }
        },
        "peers": {"rules": [
            {"id": "to-creator", "relation": "creator", "outcome": "allow"},
            {"id": "creator-peers", "relation": "creator_peer", "outcome": "allow"},
            {"id": "batch", "relation": "same_batch", "outcome": "allow"},
            {"id": "sensitive", "target": {"labels": {"team": "core"}},
             "outcome": "require_approval", "reason": "a core team member"}
        ], "default": "deny"},
        "naming": {"reserved": ["admin"]},
        "approval": {"rules": [
            {"change": ["create"], "creator": {"kind": "agent"},
             "outcome": {"kind": "notify", "approver": {"id": "owner"}}},
            {"change": ["create"], "creator": {"kind": "host"}, "outcome": {"kind": "allow"}},
            {"change": ["capability", "peers", "authority"],
             "outcome": {"kind": "require_approval", "approver": {"id": "owner"}}},
            {"change": ["descriptive", "remove"],
             "outcome": {"kind": "notify", "approver": {"id": "owner"}}},
            {"change": ["rebind"], "creator": {"kind": "host"}, "outcome": {"kind": "allow"}}
        ], "default": {"kind": "deny"}},
        "console": {"templates": ["research"]}
    })
}

fn parse(value: &Value) -> IdentityCreationPolicy {
    IdentityCreationPolicy::parse(value)
        .expect("a valid policy")
        .expect("an enabled policy")
}

fn policy() -> IdentityCreationPolicy {
    parse(&policy_json())
}

fn refusal_of(value: &Value) -> PolicyRefusal {
    IdentityCreationPolicy::parse(value).expect_err("a refused policy")
}

fn strings<const N: usize>(values: [&str; N]) -> BTreeSet<String> {
    values.into_iter().map(str::to_owned).collect()
}

fn profiles() -> MobProfiles {
    BTreeMap::from([
        (
            "assistant".to_owned(),
            ProfileFacts::new("claude-opus-5-5", Sha256Digest::of(b"assistant")),
        ),
        (
            "analyst".to_owned(),
            ProfileFacts::new("gpt-5.5", Sha256Digest::of(b"analyst")),
        ),
        (
            "lead".to_owned(),
            ProfileFacts::new("claude-opus-5-5", Sha256Digest::of(b"lead")),
        ),
    ])
}

fn mob_profile_names() -> BTreeSet<String> {
    strings(["assistant", "analyst", "lead"])
}

fn lead() -> RequesterFacts {
    RequesterFacts::agent(
        "lead-1",
        Some("lead".to_owned()),
        BTreeMap::new(),
        strings(["notes_read", "search"]),
    )
}

fn peers() -> BTreeMap<String, PeerFacts> {
    BTreeMap::from([
        (
            "lead-1".to_owned(),
            PeerFacts::new(Some("lead".to_owned()), BTreeMap::new()),
        ),
        (
            "analyst-1".to_owned(),
            PeerFacts::new(Some("analyst".to_owned()), BTreeMap::new()).wired_to_creator(),
        ),
        (
            "core-1".to_owned(),
            PeerFacts::new(
                Some("assistant".to_owned()),
                BTreeMap::from([("team".to_owned(), "core".to_owned())]),
            )
            .wired_to_creator(),
        ),
        (
            "stranger".to_owned(),
            PeerFacts::new(Some("assistant".to_owned()), BTreeMap::new()),
        ),
    ])
}

fn spec(slug: &str) -> CreateIdentitySpec {
    CreateIdentitySpec {
        slug: slug.to_owned(),
        template: "research".to_owned(),
        display_name: "Helper".to_owned(),
        purpose: "Find sources.".to_owned(),
        instructions: "Read the notes.\nReport daily.".to_owned(),
        ..CreateIdentitySpec::default()
    }
}

fn create(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    specs: &[CreateIdentitySpec],
) -> Result<NormalizedRequest, IdentityRefusal> {
    create_with(policy, requester, RequestEnvelope::default(), specs)
}

fn create_with(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    envelope: RequestEnvelope,
    specs: &[CreateIdentitySpec],
) -> Result<NormalizedRequest, IdentityRefusal> {
    let profiles = profiles();
    let peers = peers();
    let context = RequestContext {
        requester,
        profiles: &profiles,
        peers: &peers,
    };
    normalize_create(policy, &context, envelope, specs)
}

fn render(request: &IdentityRequest) -> Rendering {
    render_v1(request, None).expect("a rendering")
}

fn part_size(bytes: u32) -> Option<NonZeroU32> {
    NonZeroU32::new(bytes)
}

/// The decide-side checks of the durable decision owner, in pure form: the
/// presented digest must be the subject digest, and the presentation must
/// show every canonical part.
fn decide(
    rendering: &Rendering,
    presented: &Sha256Digest,
    max_part_bytes: Option<NonZeroU32>,
    parts_shown: &[CanonicalPart],
) -> Result<(), IdentityRefusalKind> {
    if *presented != rendering.subject_digest() {
        return Err(IdentityRefusalKind::SubjectChanged);
    }
    if !shows_every_part(rendering.as_str(), max_part_bytes, parts_shown) {
        return Err(IdentityRefusalKind::PresentationIncomplete);
    }
    Ok(())
}

fn binding_digest() -> Sha256Digest {
    Sha256Digest::of(b"binding")
}

fn golden_spec() -> CreatedSpec {
    CreatedSpec {
        template: "research".to_owned(),
        binding_digest: binding_digest(),
        profile: "assistant".to_owned(),
        model: "claude-opus-5-5".to_owned(),
        display_name: "Helper".to_owned(),
        purpose: "Find sources.".to_owned(),
        instructions: "Read the notes.\nReport daily.".to_owned(),
        supplied_context: Some(BTreeMap::from([
            ("topic".to_owned(), json!("stars")),
            ("depth".to_owned(), json!(2)),
        ])),
        descriptive_labels: BTreeMap::from([("color".to_owned(), "blue".to_owned())]),
        authority_fields: BTreeMap::from([("clearance".to_owned(), "elevated".to_owned())]),
        tools: strings(["notes_read", "search"]),
        peers: BTreeMap::from([
            ("lead-1".to_owned(), EdgeBasis::Creator),
            (
                "core-1".to_owned(),
                EdgeBasis::Rule {
                    id: "sensitive".to_owned(),
                },
            ),
        ]),
        kickoff: None,
    }
}

fn golden_create() -> IdentityRequest {
    IdentityRequest {
        requester: Requester::Agent {
            identity: "lead-1".to_owned(),
        },
        proposer: None,
        source_evidence: None,
        action: RequestAction::Create {
            identities: BTreeMap::from([("team:helper".to_owned(), golden_spec())]),
        },
        consequences: BTreeSet::from([
            Consequence {
                identity: "team:helper".to_owned(),
                effect: ConsequenceEffect::AuthorityField {
                    key: "clearance".to_owned(),
                    value: "elevated".to_owned(),
                },
            },
            Consequence {
                identity: "team:helper".to_owned(),
                effect: ConsequenceEffect::PeerNeedsApproval {
                    peer: "core-1".to_owned(),
                    rule: Some("sensitive".to_owned()),
                    reason: Some("a core team member".to_owned()),
                },
            },
        ]),
    }
}

fn golden_update() -> IdentityRequest {
    IdentityRequest {
        requester: Requester::Agent {
            identity: "lead-1".to_owned(),
        },
        proposer: None,
        source_evidence: None,
        action: RequestAction::Update(Box::new(UpdatedSpec {
            identity: "team:helper".to_owned(),
            expected_revision: 3,
            template: "research".to_owned(),
            binding_digest: binding_digest(),
            rebind: None,
            display_name: None,
            purpose: Some("Find better sources.".to_owned()),
            instructions: None,
            supplied_context: None,
            descriptive_labels: None,
            authority_fields: None,
            tools: Some(strings(["search"])),
            peers_add: BTreeMap::from([("analyst-1".to_owned(), EdgeBasis::CreatorPeer)]),
            peers_remove: strings(["core-1"]),
        })),
        consequences: BTreeSet::new(),
    }
}

fn golden_remove() -> IdentityRequest {
    IdentityRequest {
        requester: Requester::Host {
            surface: CONSOLE_SURFACE.to_owned(),
        },
        proposer: Some("op-proposal-7".to_owned()),
        source_evidence: Some(SourceEvidence::new(
            "adapter-1",
            "user-9",
            "event:42",
            1_730_000_000_000,
        )),
        action: RequestAction::Remove {
            identity: "team:helper".to_owned(),
            expected_revision: 3,
        },
        consequences: BTreeSet::new(),
    }
}

fn lines(texts: &[&str]) -> Vec<ConsequenceLine> {
    texts
        .iter()
        .map(|text| ConsequenceLine::new(*text))
        .collect()
}

fn created_spec<'a>(request: &'a IdentityRequest, identity: &str) -> &'a CreatedSpec {
    match &request.action {
        RequestAction::Create { identities } => identities.get(identity).expect("the identity"),
        RequestAction::Update(_) | RequestAction::Remove { .. } => panic!("a create request"),
    }
}

fn created_spec_mut<'a>(request: &'a mut IdentityRequest, identity: &str) -> &'a mut CreatedSpec {
    match &mut request.action {
        RequestAction::Create { identities } => identities.get_mut(identity).expect("the identity"),
        RequestAction::Update(_) | RequestAction::Remove { .. } => panic!("a create request"),
    }
}

// ---------------------------------------------------------------------------
// Policy validation rows
// ---------------------------------------------------------------------------

#[test]
fn a_valid_policy_parses_with_the_contract_defaults() {
    let bare = parse(&json!({"enabled": true}));
    let limits = bare.limits();
    assert!(limits.total.is_unlimited());
    assert!(limits.per_creator.is_unlimited());
    assert!(limits.peers_per_identity.is_unlimited());
    assert_eq!(limits.batch.value(), 32);
    assert_eq!(limits.open_decisions_per_creator.value(), 3);
    let timeouts = bare.approval().timeouts();
    assert_eq!(timeouts.create, 86_400_000);
    assert_eq!(timeouts.capability, 14_400_000);
    assert_eq!(timeouts.peers, 14_400_000);
    assert_eq!(timeouts.max, MAX_DECISION_TIMEOUT_MS);
    assert_eq!(bare.approval().default_outcome().kind(), ApprovalKind::Deny);
    assert_eq!(bare.peers().default_outcome(), PeerOutcome::Deny);
    assert_eq!(bare.console(), &ConsoleCreation::Disabled);
    assert!(bare.templates().is_empty());

    let policy = policy();
    let research = policy.template("research").expect("research");
    assert_eq!(research.excess(), ExcessMode::Strict);
    assert_eq!(research.kickoff(), KickoffRule::Allowed);
    assert_eq!(research.preparation(), PreparationMode::None);
    assert_eq!(research.max_bytes().display_name, 64);
    assert_eq!(research.max_bytes().supplied_context, 16_384);
    assert_eq!(research.models(), &ModelChoice::ProfileModel);
    assert_eq!(
        research
            .callbacks()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["notes_read", "notes_write"]
    );
    assert_eq!(
        research.callbacks()["notes_read"],
        callback_definition_digest(
            "notes_read",
            Some("Read notes"),
            Some(&json!({"type": "object"}))
        )
    );
    assert_eq!(
        research.callbacks()["notes_write"],
        callback_definition_digest("notes_write", None, None),
        "a bare name declares no description and no schema"
    );
    assert_eq!(
        policy.template("open").map(Template::profiles),
        Some(&ProfileChoice::Any)
    );
    assert!(policy.is_reserved_label("team"));
    assert!(
        policy.is_reserved_label("role"),
        "Meta's own keys are reserved"
    );
    assert!(policy.is_reserved_context("project"));
    assert_eq!(
        policy.creator_templates("lead-1", Some("lead"), &BTreeMap::new()),
        strings(["open", "research"])
    );
    assert!(
        policy
            .creator_templates("lead-1", Some("assistant"), &BTreeMap::new())
            .is_empty()
    );
}

#[test]
fn unknown_policy_keys_are_refused_typed() {
    let cases = [
        (json!({"enabled": true, "surprise": 1}), "", "surprise"),
        (
            json!({"enabled": true, "console": {"inherit_creator_access": true}}),
            "console",
            "inherit_creator_access",
        ),
        (
            json!({"enabled": true, "limits": {"totals": 3}}),
            "limits",
            "totals",
        ),
        (
            json!({"enabled": true, "templates": {"t": {
                "profiles": "any", "namespace": "t", "tools": {"celing": []}}}}),
            "templates.t.tools",
            "celing",
        ),
        (
            json!({"enabled": true, "templates": {"t": {"profiles": "every", "namespace": "t"}}}),
            "templates.t.profiles",
            "\"every\"",
        ),
        (
            json!({"enabled": true, "templates": {"t": {"profiles": 5, "namespace": "t"}}}),
            "templates.t.profiles",
            "a list of profile names or \"any\"",
        ),
        (
            json!({"enabled": true, "templates": {"t": {"profiles": "any"}}}),
            "templates.t",
            "namespace",
        ),
        (
            json!({"enabled": true, "creators": [{"id": "a", "templats": []}]}),
            "creators[0]",
            "templats",
        ),
        (
            json!({"enabled": true, "approval": {"rules": [{"change": ["create"]}, {"chnage": []}]}}),
            "approval.rules[1]",
            "chnage",
        ),
        (json!({"enabled": "yes"}), "enabled", "boolean"),
    ];
    for (value, path, detail) in cases {
        let refusal = refusal_of(&value);
        let PolicyRefusalKind::Malformed { detail: shown } = &refusal.kind else {
            panic!("{value}: {refusal}");
        };
        assert_eq!(refusal.path, path, "{value}: {refusal}");
        assert!(shown.contains(detail), "{value}: {refusal}");
        assert!(!shown.contains("ListOrKeyword"), "{refusal}");
        assert!(refusal.to_string().starts_with("identity_creation"));
    }
}

#[test]
fn a_disabled_policy_is_validated_and_yields_no_policy() {
    assert_eq!(IdentityCreationPolicy::parse(&json!({})).unwrap(), None);
    let mut disabled = policy_json();
    disabled["enabled"] = json!(false);
    assert_eq!(IdentityCreationPolicy::parse(&disabled).unwrap(), None);
    assert_eq!(
        RuntimeOptionPolicy::parse(&disabled).unwrap(),
        RuntimeOptionPolicy::Disabled
    );
    disabled["templates"]["research"]["tools"]["floor"] = json!(["shell"]);
    let refusal = refusal_of(&disabled);
    assert_eq!(refusal.path, "templates.research.tools.floor");
    assert_eq!(
        refusal.kind,
        PolicyRefusalKind::FloorOutsideCeiling {
            tool: "shell".to_owned()
        }
    );
}

type PolicyChange = fn(&mut Value);

fn policy_case(
    change: PolicyChange,
    path: &'static str,
    kind: PolicyRefusalKind,
) -> (PolicyChange, &'static str, PolicyRefusalKind) {
    (change, path, kind)
}

#[test]
fn policy_validation_refuses_typed() {
    let cases = [
        policy_case(
            |value| value["templates"]["research"]["tools"]["floor"] = json!(["shell"]),
            "templates.research.tools.floor",
            PolicyRefusalKind::FloorOutsideCeiling {
                tool: "shell".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["tools"]["ceiling"] =
                    json!(["notes_read", "identity_create"]);
            },
            "templates.research.tools.ceiling",
            PolicyRefusalKind::CreationToolWithoutGrant {
                tool: "identity_create".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["authority_fields"] =
                    json!({"mood": {"values": ["calm"]}});
            },
            "templates.research.authority_fields.mood",
            PolicyRefusalKind::AuthorityFieldNotReserved {
                key: "mood".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["authority_fields"] =
                    json!({"clearance": {"schema": {"type": "string"}}});
            },
            "templates.research.authority_fields.clearance.schema",
            PolicyRefusalKind::Unsupported {
                feature: "an authority field schema".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["authority_fields"] =
                    json!({"clearance": {"values": []}});
            },
            "templates.research.authority_fields.clearance.values",
            PolicyRefusalKind::Empty,
        ),
        policy_case(
            |value| value["creators"][0]["templates"] = json!(["nope"]),
            "creators[0].templates[0]",
            PolicyRefusalKind::UnknownTemplate {
                template: "nope".to_owned(),
            },
        ),
        policy_case(
            |value| value["creators"][0]["templates"] = json!([]),
            "creators[0].templates",
            PolicyRefusalKind::Empty,
        ),
        policy_case(
            |value| value["templates"]["research"]["callbacks"] = json!([{"name": "notes read"}]),
            "templates.research.callbacks[0].name",
            PolicyRefusalKind::Invalid {
                value: "notes read".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["callbacks"] =
                    json!([{"name": "notes_read", "execution": {"kind": "fast"}}]);
            },
            "templates.research.callbacks[0].execution",
            PolicyRefusalKind::Unsupported {
                feature: "a callback tool's execution in a template".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["callbacks"] =
                    json!([{"name": "notes_read", "descripton": "Read notes"}]);
            },
            "templates.research.callbacks[0].descripton",
            PolicyRefusalKind::Malformed {
                detail: "unknown field `descripton`".to_owned(),
            },
        ),
        policy_case(
            |value| value["templates"]["research"]["preamble"] = json!("Hello {name}"),
            "templates.research.preamble",
            PolicyRefusalKind::InvalidPlaceholder {
                placeholder: "{name}".to_owned(),
            },
        ),
        policy_case(
            |value| value["approval"]["rules"][3]["outcome"] = json!({"kind": "notify"}),
            "approval.rules[3].outcome.approver",
            PolicyRefusalKind::MissingApprover,
        ),
        policy_case(
            |value| {
                value["approval"]["rules"][1]["outcome"] =
                    json!({"kind": "allow", "approver": {"id": "owner"}});
            },
            "approval.rules[1].outcome.approver",
            PolicyRefusalKind::UnexpectedApprover,
        ),
        policy_case(
            |value| {
                value["approval"]["rules"][0]["outcome"] =
                    json!({"kind": "review", "approver": {"id": "owner"}});
            },
            "approval.rules[0].outcome.reviewer",
            PolicyRefusalKind::MissingReviewer,
        ),
        policy_case(
            |value| {
                value["approval"]["rules"][0]["outcome"] = json!({
                    "kind": "review", "approver": {"id": "owner"},
                    "reviewer": "reviewer-1", "max_rounds": 0
                });
            },
            "approval.rules[0].outcome.max_rounds",
            PolicyRefusalKind::OutOfRange {
                value: 0,
                min: 1,
                max: 16,
            },
        ),
        policy_case(
            |value| {
                value["approval"]["rules"][3]["outcome"] =
                    json!({"kind": "notify", "approver": {"id": "owner", "label": "team"}});
            },
            "approval.rules[3].outcome.approver",
            PolicyRefusalKind::InvalidApprover,
        ),
        policy_case(
            |value| {
                value["approval"]["rules"][2]["outcome"] =
                    json!({"kind": "notify", "approver": {"id": "owner"}});
            },
            "approval.rules[2].outcome.kind",
            PolicyRefusalKind::AuthorityChangeNeedsHumanDecision,
        ),
        policy_case(
            |value| value["approval"]["timeouts_ms"] = json!({"max": 1000, "create": 2000}),
            "approval.timeouts_ms.create",
            PolicyRefusalKind::OutOfRange {
                value: 2000,
                min: 1,
                max: 1000,
            },
        ),
        policy_case(
            |value| value["approval"]["timeouts_ms"] = json!({"max": MAX_DECISION_TIMEOUT_MS + 1}),
            "approval.timeouts_ms.max",
            PolicyRefusalKind::OutOfRange {
                value: MAX_DECISION_TIMEOUT_MS + 1,
                min: 1,
                max: MAX_DECISION_TIMEOUT_MS,
            },
        ),
        policy_case(
            |value| value["peers"]["rules"][0]["id"] = json!("default"),
            "peers.rules[0].id",
            PolicyRefusalKind::ReservedRuleId {
                id: "default".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["creators"] = json!([
                    {"id": "a", "templates": ["research"]},
                    {"id": "a", "templates": ["open"]}
                ]);
            },
            "creators[1].id",
            PolicyRefusalKind::Duplicate {
                value: "a".to_owned(),
            },
        ),
        policy_case(
            |value| value["templates"]["research"]["namespace"] = json!("Team"),
            "templates.research.namespace",
            PolicyRefusalKind::Invalid {
                value: "Team".to_owned(),
            },
        ),
        policy_case(
            |value| value["naming"]["pattern"] = json!("{namespace}"),
            "naming.pattern",
            PolicyRefusalKind::Invalid {
                value: "{namespace}".to_owned(),
            },
        ),
        policy_case(
            |value| value["naming"]["pattern"] = json!("{slug}/{namespace}"),
            "naming.pattern",
            PolicyRefusalKind::Invalid {
                value: "{slug}/{namespace}".to_owned(),
            },
        ),
        policy_case(
            |value| value["naming"]["slug"] = json!({"min_bytes": 5, "max_bytes": 4}),
            "naming.slug.max_bytes",
            PolicyRefusalKind::OutOfRange {
                value: 4,
                min: 5,
                max: 128,
            },
        ),
        policy_case(
            |value| value["templates"]["research"]["max_bytes"]["purpose"] = json!(0),
            "templates.research.max_bytes.purpose",
            PolicyRefusalKind::OutOfRange {
                value: 0,
                min: 1,
                max: 262_144,
            },
        ),
        policy_case(
            |value| value["templates"]["research"]["preparation"] = json!("constructed"),
            "templates.research.construction_deadline_ms",
            PolicyRefusalKind::Empty,
        ),
        policy_case(
            |value| value["templates"]["research"]["construction_deadline_ms"] = json!(5000),
            "templates.research.construction_deadline_ms",
            PolicyRefusalKind::Invalid {
                value: "5000".to_owned(),
            },
        ),
        policy_case(
            |value| value["templates"]["research"]["profiles"] = json!([]),
            "templates.research.profiles",
            PolicyRefusalKind::Empty,
        ),
        policy_case(
            |value| value["templates"]["research"]["profiles"] = json!(["assistant", "assistant"]),
            "templates.research.profiles[1]",
            PolicyRefusalKind::Duplicate {
                value: "assistant".to_owned(),
            },
        ),
        policy_case(
            |value| value["console"]["templates"] = json!(["nope"]),
            "console.templates[0]",
            PolicyRefusalKind::UnknownTemplate {
                template: "nope".to_owned(),
            },
        ),
        policy_case(
            |value| value["limits"]["batch"] = json!(CORE_BATCH_MAX + 1),
            "limits.batch",
            PolicyRefusalKind::OutOfRange {
                value: CORE_BATCH_MAX + 1,
                min: 0,
                max: CORE_BATCH_MAX,
            },
        ),
        policy_case(
            |value| value["reserved_keys"]["labels"] = json!(["team", "team"]),
            "reserved_keys.labels[1]",
            PolicyRefusalKind::Duplicate {
                value: "team".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["templates"]["research"]["callbacks"] =
                    json!(["notes_read", {"name": "notes_read", "description": "Again"}]);
            },
            "templates.research.callbacks[1]",
            PolicyRefusalKind::Duplicate {
                value: "notes_read".to_owned(),
            },
        ),
    ];
    for (change, path, kind) in cases {
        let mut value = policy_json();
        change(&mut value);
        let refusal = refusal_of(&value);
        assert_eq!(refusal.path, path, "{refusal}");
        assert_eq!(refusal.kind, kind, "{refusal}");
    }
}

#[test]
fn creation_tools_need_a_template_that_grants_creation() {
    let mut value = policy_json();
    value["templates"]["research"]["tools"]["ceiling"] =
        json!(["notes_read", "search", "identity_create"]);
    assert!(IdentityCreationPolicy::parse(&value).is_err());
    value["templates"]["research"]["may_grant_creation"] = json!(true);
    let policy = parse(&value);
    let research = policy.template("research").expect("research");
    assert!(research.may_grant_creation());
    assert!(research.ceiling().contains("identity_create"));
}

#[test]
fn every_limit_takes_zero_as_unlimited() {
    let mut value = policy_json();
    value["limits"] = json!({
        "total": 0, "per_creator": 0, "peers_per_identity": 0,
        "batch": 0, "open_decisions_per_creator": 0
    });
    let unlimited = parse(&value);
    let limits = unlimited.limits();
    for limit in [
        limits.total,
        limits.per_creator,
        limits.peers_per_identity,
        limits.batch,
        limits.open_decisions_per_creator,
    ] {
        assert!(limit.is_unlimited());
        assert!(!limit.is_reached_by(u64::MAX));
        assert!(!limit.is_exceeded_by(u64::MAX));
    }
    // Batch 0 sets no policy limit: five identities pass where the fixture's
    // batch of 4 refuses them.
    let five: Vec<CreateIdentitySpec> = (0..5).map(|n| spec(&format!("helper-{n}"))).collect();
    let host = RequesterFacts::host("gateway");
    assert!(create(&unlimited, &host, &five).is_ok());
    let refusal = create(&policy(), &host, &five).expect_err("over the batch");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::LimitReached {
            limit: LimitKind::Batch
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("identities"));

    value["limits"] = json!({"total": 2, "peers_per_identity": 1});
    let limited = parse(&value);
    assert!(!limited.limits().total.is_reached_by(1));
    assert!(limited.limits().total.is_reached_by(2));
    assert!(limited.limits().total.is_exceeded_by(3));
    let mut two_peers = spec("helper");
    two_peers.peers = vec!["lead-1".to_owned(), "analyst-1".to_owned()];
    let refusal = create(&limited, &lead(), &[two_peers]).expect_err("over the peer limit");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::LimitReached {
            limit: LimitKind::PeersPerIdentity
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("identities[0].peers"));
}

#[test]
fn a_batch_above_cores_transaction_maximum_is_refused() {
    let mut value = policy_json();
    value["limits"]["batch"] = json!(0);
    let policy = parse(&value);
    let specs: Vec<CreateIdentitySpec> = (0..=CORE_BATCH_MAX)
        .map(|n| spec(&format!("helper-{n}")))
        .collect();
    let refusal = create(&policy, &RequesterFacts::host("gateway"), &specs)
        .expect_err("over the transaction maximum");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::BatchTooLarge {
            max: CORE_BATCH_MAX
        }
    );
    assert_eq!(refusal.to_data()["kind"], "batch_too_large");
    assert_eq!(refusal.to_data()["max"], CORE_BATCH_MAX);
}

#[test]
fn template_profiles_must_exist_in_the_mob_definition() {
    let policy = policy();
    assert!(policy.check_mob_profiles(&mob_profile_names()).is_ok());
    let refusal = policy
        .check_mob_profiles(&strings(["assistant"]))
        .expect_err("analyst is missing");
    assert_eq!(refusal.path, "templates.research.profiles[1]");
    assert_eq!(
        refusal.kind,
        PolicyRefusalKind::UnknownProfile {
            profile: "analyst".to_owned()
        }
    );
}

#[test]
fn the_policy_comes_from_the_option_or_the_host_never_both() {
    let policy = policy();
    let mob = mob_profile_names();
    let from_option = resolve_launch_policy(
        None,
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
        &mob,
        true,
    )
    .unwrap();
    assert_eq!(from_option.as_deref(), Some(&policy));
    let from_host = resolve_launch_policy(
        Some(Ok(Some(policy.clone()))),
        RuntimeOptionPolicy::Absent,
        &mob,
        true,
    )
    .unwrap();
    assert_eq!(from_host.as_deref(), Some(&policy));
    let host_without_one = resolve_launch_policy(
        Some(Ok(None)),
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
        &mob,
        true,
    )
    .unwrap();
    assert_eq!(host_without_one.as_deref(), Some(&policy));
    for option in [
        RuntimeOptionPolicy::Disabled,
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
    ] {
        let both = resolve_launch_policy(Some(Ok(Some(policy.clone()))), option, &mob, true)
            .expect_err("two sources");
        assert_eq!(both.kind, PolicyRefusalKind::TwoSources);
    }
    let host_refusal = PolicyRefusal::new("templates", PolicyRefusalKind::Empty);
    assert_eq!(
        resolve_launch_policy(
            Some(Err(host_refusal.clone())),
            RuntimeOptionPolicy::Absent,
            &mob,
            true
        )
        .unwrap_err(),
        host_refusal
    );
    assert!(
        resolve_launch_policy(None, RuntimeOptionPolicy::Absent, &mob, true)
            .unwrap()
            .is_none()
    );
    assert!(
        resolve_launch_policy(None, RuntimeOptionPolicy::Disabled, &mob, true)
            .unwrap()
            .is_none()
    );
    let missing = resolve_launch_policy(
        None,
        RuntimeOptionPolicy::Enabled(Box::new(policy)),
        &strings(["assistant", "lead"]),
        true,
    )
    .expect_err("a profile is missing");
    assert_eq!(
        missing.kind,
        PolicyRefusalKind::UnknownProfile {
            profile: "analyst".to_owned()
        }
    );
}

#[test]
fn a_constructed_template_needs_its_deadline_and_a_native_host() {
    let mut value = policy_json();
    value["templates"]["research"]["preparation"] = json!("constructed");
    value["templates"]["research"]["construction_deadline_ms"] = json!(60_000);
    let policy = parse(&value);
    assert_eq!(policy.template_needing_native_host(), Some("research"));
    let research = policy.template("research").expect("research");
    assert_eq!(research.construction_deadline_ms(), Some(60_000));
    let refusal = resolve_launch_policy(
        None,
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
        &mob_profile_names(),
        true,
    )
    .expect_err("no native host");
    assert_eq!(refusal.path, "templates.research.preparation");
    assert_eq!(
        refusal.kind,
        PolicyRefusalKind::NeedsNativeHost {
            template: "research".to_owned()
        }
    );
    assert!(
        resolve_launch_policy(
            Some(Ok(None)),
            RuntimeOptionPolicy::Enabled(Box::new(policy)),
            &mob_profile_names(),
            true,
        )
        .unwrap()
        .is_some()
    );
    value["templates"]["research"]["construction_deadline_ms"] = json!(10);
    let refusal = refusal_of(&value);
    assert_eq!(refusal.path, "templates.research.construction_deadline_ms");
    assert!(matches!(refusal.kind, PolicyRefusalKind::OutOfRange { .. }));
}

#[test]
fn a_launch_without_persistence_refuses_a_policy_from_either_source() {
    let policy = policy();
    let mob = mob_profile_names();
    for (host_policy, option) in [
        (None, RuntimeOptionPolicy::Enabled(Box::new(policy.clone()))),
        (None, RuntimeOptionPolicy::Disabled),
        (Some(Ok(None)), RuntimeOptionPolicy::Disabled),
        (Some(Ok(Some(policy.clone()))), RuntimeOptionPolicy::Absent),
    ] {
        let refusal =
            resolve_launch_policy(host_policy, option, &mob, false).expect_err("no persistence");
        assert_eq!(refusal.kind, PolicyRefusalKind::NeedsPersistentLaunch);
        assert_eq!(refusal.path, "");
    }
    for host_policy in [None, Some(Ok(None))] {
        assert!(
            resolve_launch_policy(host_policy, RuntimeOptionPolicy::Absent, &mob, false)
                .expect("nothing given, nothing refused")
                .is_none()
        );
    }
    let both = resolve_launch_policy(
        Some(Ok(Some(policy.clone()))),
        RuntimeOptionPolicy::Enabled(Box::new(policy)),
        &mob,
        false,
    )
    .expect_err("two sources");
    assert_eq!(both.kind, PolicyRefusalKind::TwoSources);
}

#[test]
fn an_empty_console_template_list_disables_console_creation() {
    let mut value = policy_json();
    value["console"] = json!({"templates": []});
    let disabled = parse(&value);
    assert_eq!(disabled.console(), &ConsoleCreation::Disabled);
    let refusal = create(&disabled, &RequesterFacts::console(), &[spec("helper")])
        .expect_err("console creation is off");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TemplateNotAllowed {
            template: "research".to_owned()
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("identities[0].template"));
    value.as_object_mut().expect("an object").remove("console");
    assert_eq!(parse(&value).console(), &ConsoleCreation::Disabled);

    let enabled = policy();
    assert!(enabled.console().allows("research"));
    let created = create(&enabled, &RequesterFacts::console(), &[spec("helper")])
        .expect("the console creates from its templates");
    assert_eq!(created.plan.kind(), ApprovalKind::Allow);
    let mut open = spec("helper");
    open.template = "open".to_owned();
    let refusal =
        create(&enabled, &RequesterFacts::console(), &[open]).expect_err("not a console template");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TemplateNotAllowed {
            template: "open".to_owned()
        }
    );
}

#[test]
fn the_policy_revision_is_stable_and_changes_with_the_policy() {
    let first = policy();
    let second = policy();
    assert_eq!(first.revision(), second.revision());
    let mut explicit = policy_json();
    explicit["limits"]["open_decisions_per_creator"] = json!(3);
    assert_eq!(
        parse(&explicit).revision(),
        first.revision(),
        "defaults are explicit"
    );
    let mut changed = policy_json();
    changed["limits"]["total"] = json!(10);
    assert_ne!(parse(&changed).revision(), first.revision());
}

// ---------------------------------------------------------------------------
// Templates: binding, tool policy, preamble
// ---------------------------------------------------------------------------

fn research_binding(value: &Value, profiles: &MobProfiles) -> TemplateBinding {
    let policy = parse(value);
    resolve_binding(
        policy.template("research").expect("research"),
        Some("assistant"),
        None,
        profiles,
    )
    .expect("a binding")
}

type BindingChange = fn(&mut Value);

fn binding_change(change: BindingChange) -> BindingChange {
    change
}

#[test]
fn the_template_binding_digest_covers_the_sealed_template_facts() {
    let base = research_binding(&policy_json(), &profiles());
    assert_eq!(base.model, "claude-opus-5-5");
    assert_eq!(base.accounts, strings(["binding:notes"]));
    assert_eq!(base.floor, strings(["notes_read"]));
    assert_eq!(base.preparation, PreparationMode::None);
    assert_eq!(base.digest(), base.clone().digest());

    let unchanged = [
        binding_change(|value| value["templates"]["research"]["title"] = json!("Another title")),
        binding_change(|value| value["templates"]["research"]["description"] = json!("Described")),
        binding_change(|value| {
            value["templates"]["research"]["preamble"] = json!("Hello {creator}.");
        }),
        binding_change(|value| value["templates"]["research"]["max_bytes"]["purpose"] = json!(10)),
        binding_change(|value| value["templates"]["research"]["excess"] = json!("consequence")),
        binding_change(|value| value["templates"]["research"]["kickoff"] = json!("forbidden")),
    ];
    for change in unchanged {
        let mut value = policy_json();
        change(&mut value);
        assert_eq!(
            research_binding(&value, &profiles()).digest(),
            base.digest()
        );
    }

    let changed = [
        binding_change(|value| {
            value["templates"]["research"]["tools"]["ceiling"] = json!(["notes_read", "web"]);
        }),
        binding_change(|value| value["templates"]["research"]["tools"]["floor"] = json!([])),
        binding_change(|value| {
            value["templates"]["research"]["accounts"] = json!(["binding:other"]);
        }),
        binding_change(|value| value["templates"]["research"]["version"] = json!("2")),
        binding_change(|value| value["templates"]["research"]["preparation"] = json!("required")),
        binding_change(|value| value["templates"]["research"]["callbacks"] = json!([])),
        binding_change(|value| {
            value["templates"]["research"]["callbacks"] = json!(["notes_read", "notes_write"]);
        }),
        binding_change(|value| {
            value["templates"]["research"]["callbacks"][1] =
                json!({"name": "notes_write", "description": "Write notes"});
        }),
    ];
    let mut digests = BTreeSet::from([base.digest()]);
    for change in changed {
        let mut value = policy_json();
        change(&mut value);
        let digest = research_binding(&value, &profiles()).digest();
        assert_ne!(digest, base.digest());
        assert!(digests.insert(digest), "each change gives its own digest");
    }

    let mut other_profile = profiles();
    other_profile.insert(
        "assistant".to_owned(),
        ProfileFacts::new("claude-opus-5-5", Sha256Digest::of(b"assistant v2")),
    );
    assert_ne!(
        research_binding(&policy_json(), &other_profile).digest(),
        base.digest()
    );
    let analyst = resolve_binding(
        policy().template("research").expect("research"),
        Some("analyst"),
        None,
        &profiles(),
    )
    .expect("a binding");
    assert_eq!(analyst.model, "gpt-5.5");
    assert_ne!(analyst.digest(), base.digest());
}

#[test]
fn profile_facts_digest_the_inline_profile() {
    let profile: meerkat_mob::Profile =
        serde_json::from_value(json!({"model": "claude-opus-5-5"})).expect("a profile");
    let facts = ProfileFacts::of_profile(&profile).expect("facts");
    assert_eq!(facts.model, "claude-opus-5-5");
    assert_eq!(ProfileFacts::of_profile(&profile).expect("facts"), facts);
    let explicit: meerkat_mob::Profile = serde_json::from_value(json!({
        "model": "claude-opus-5-5", "external_addressable": false, "skills": []
    }))
    .expect("a profile");
    assert_eq!(
        ProfileFacts::of_profile(&explicit).expect("facts").digest,
        facts.digest
    );
    let other: meerkat_mob::Profile = serde_json::from_value(json!({
        "model": "claude-opus-5-5", "external_addressable": true
    }))
    .expect("a profile");
    assert_ne!(
        ProfileFacts::of_profile(&other).expect("facts").digest,
        facts.digest
    );
}

#[test]
fn profile_facts_detect_changes_inside_option_and_schema_values() {
    let digest = |value: Value| {
        let profile: meerkat_mob::Profile = serde_json::from_value(value).expect("a profile");
        ProfileFacts::of_profile(&profile).expect("facts").digest
    };
    let schema = json!({"type": "object", "properties": {"a": {"type": "string"}}});
    let mut closed = schema.clone();
    closed["additionalProperties"] = json!(false);
    assert_ne!(
        digest(json!({"model": "gpt-5.5", "output_schema": closed})),
        digest(json!({"model": "gpt-5.5", "output_schema": schema})),
        "removing additionalProperties false changes the digest"
    );
    assert_ne!(
        digest(json!({
            "model": "gpt-5.5",
            "provider_params": {"provider_tag": {"provider": "open_ai", "prompt_cache_enabled": false}}
        })),
        digest(json!({
            "model": "gpt-5.5",
            "provider_params": {"provider_tag": {"provider": "open_ai"}}
        })),
        "removing prompt_cache_enabled false changes the digest"
    );
    assert_ne!(
        digest(json!({"model": "gpt-5.5"})),
        digest(json!({"model": "gpt-5.4"})),
        "the model is always digested"
    );
}

#[test]
fn profile_and_model_resolve_from_the_template() {
    let policy = policy();
    let research = policy.template("research").expect("research");
    let open = policy.template("open").expect("open");
    let profiles = profiles();
    let default = resolve_binding(research, None, None, &profiles).expect("a default");
    assert_eq!(default.profile, "assistant");
    assert_eq!(default.model, "claude-opus-5-5");
    assert_eq!(
        resolve_binding(research, Some("assistant"), Some("gpt-5.5"), &profiles).unwrap_err(),
        IdentityRefusalKind::ModelNotAllowed {
            model: "gpt-5.5".to_owned()
        }
    );
    assert_eq!(
        resolve_binding(research, Some("lead"), None, &profiles).unwrap_err(),
        IdentityRefusalKind::ProfileNotAllowed {
            profile: Some("lead".to_owned())
        }
    );
    assert_eq!(
        resolve_binding(open, None, None, &profiles).unwrap_err(),
        IdentityRefusalKind::ProfileNotAllowed { profile: None }
    );
    assert_eq!(
        resolve_binding(open, Some("ghost"), None, &profiles).unwrap_err(),
        IdentityRefusalKind::ProfileNotAllowed {
            profile: Some("ghost".to_owned())
        }
    );
    assert_eq!(
        resolve_binding(open, Some("lead"), None, &profiles)
            .expect("any profile")
            .profile,
        "lead"
    );

    let mut value = policy_json();
    value["templates"]["research"]["models"] = json!(["model-a", "model-b"]);
    let listed = parse(&value);
    let research = listed.template("research").expect("research");
    assert_eq!(
        resolve_binding(research, None, None, &profiles)
            .expect("a default model")
            .model,
        "model-a"
    );
    assert_eq!(
        resolve_binding(research, None, Some("model-b"), &profiles)
            .expect("a listed model")
            .model,
        "model-b"
    );
    assert!(resolve_binding(research, None, Some("model-c"), &profiles).is_err());
}

#[test]
fn the_build_check_compares_the_given_callbacks_with_the_sealed_ones() {
    let binding = research_binding(&policy_json(), &profiles());
    let schema = json!({"type": "object"});
    let sealed = [
        ("notes_read", Some("Read notes"), Some(&schema)),
        ("notes_write", None, None),
    ];
    assert_eq!(check_build_callbacks(&binding, sealed), Ok(()));
    let reversed = [
        ("notes_write", None, None),
        ("notes_read", Some("Read notes"), Some(&schema)),
    ];
    assert_eq!(
        check_build_callbacks(&binding, reversed),
        Ok(()),
        "the order a build gives them in is not part of the binding"
    );

    // Anything but exactly the sealed definitions is a changed binding.
    let other_schema = json!({"type": "object", "required": ["id"]});
    let changed: [&[(&str, Option<&str>, Option<&Value>)]; 6] = [
        &[
            ("notes_read", Some("Read every note"), Some(&schema)),
            ("notes_write", None, None),
        ],
        &[
            ("notes_read", Some("Read notes"), Some(&other_schema)),
            ("notes_write", None, None),
        ],
        &[
            ("notes_read", Some("Read notes"), None),
            ("notes_write", None, None),
        ],
        &[("notes_read", Some("Read notes"), Some(&schema))],
        &[
            ("notes_read", Some("Read notes"), Some(&schema)),
            ("notes_write", None, None),
            ("notes_delete", None, None),
        ],
        &[
            ("notes_read", Some("Read notes"), Some(&schema)),
            ("notes_write", None, None),
            ("notes_write", None, None),
        ],
    ];
    for (index, given) in changed.into_iter().enumerate() {
        assert_eq!(
            check_build_callbacks(&binding, given.iter().copied()),
            Err(IdentityRefusalKind::TemplateBindingChanged),
            "case {index}"
        );
    }
}

#[test]
fn a_changed_registered_callback_definition_changes_the_binding_digest() {
    let base = research_binding(&policy_json(), &profiles());
    assert_eq!(
        base.callbacks
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["notes_read", "notes_write"]
    );

    // The declaration's order changes nothing.
    let mut reordered = policy_json();
    let declared = reordered["templates"]["research"]["callbacks"]
        .as_array_mut()
        .expect("a list");
    declared.reverse();
    assert_eq!(
        research_binding(&reordered, &profiles()).digest(),
        base.digest()
    );

    // A changed description or input schema of a declared callback changes
    // the digest, each change its own; the other callback keeps its digest.
    let changes = [
        binding_change(|value| {
            value["templates"]["research"]["callbacks"][0]["description"] =
                json!("Read every note");
        }),
        binding_change(|value| {
            value["templates"]["research"]["callbacks"][0]["input_schema"] =
                json!({"type": "object", "required": ["id"]});
        }),
    ];
    let mut digests = BTreeSet::from([base.digest()]);
    for change in changes {
        let mut value = policy_json();
        change(&mut value);
        let binding = research_binding(&value, &profiles());
        assert_ne!(
            binding.callbacks["notes_read"],
            base.callbacks["notes_read"]
        );
        assert_eq!(
            binding.callbacks["notes_write"],
            base.callbacks["notes_write"]
        );
        assert!(
            digests.insert(binding.digest()),
            "each change gives its own digest"
        );
    }

    // An identity sealed under the old declaration reads a changed binding:
    // its updates are refused until the host rebinds it to the new one.
    let mut redescribed = policy_json();
    redescribed["templates"]["research"]["callbacks"][0]["description"] = json!("Read every note");
    let redescribed = parse(&redescribed);
    let entry = entry_facts(current_binding());
    assert_eq!(entry.binding_digest, base.digest());
    let purpose = update_spec(UpdateSetSpec {
        purpose: Some("Find better sources.".to_owned()),
        ..UpdateSetSpec::default()
    });
    assert_eq!(
        update_with(&redescribed, &lead(), &entry, &purpose)
            .unwrap_err()
            .kind,
        IdentityRefusalKind::TemplateBindingChanged
    );
    let rebind = UpdateIdentitySpec {
        rebind: true,
        ..update_spec(UpdateSetSpec::default())
    };
    let normalized = update_with(
        &redescribed,
        &RequesterFacts::host("gateway"),
        &entry,
        &rebind,
    )
    .expect("the host rebinds");
    let RequestAction::Update(updated) = &normalized.request.action else {
        panic!("an update");
    };
    let rebound = resolve_binding(
        redescribed.template("research").expect("research"),
        Some("assistant"),
        None,
        &profiles(),
    )
    .expect("a binding");
    assert_eq!(updated.rebind, Some(rebound.digest()));
}

fn tool_binding(floor: &[&str], ceiling: &[&str]) -> TemplateBinding {
    let mut value = policy_json();
    value["templates"]["research"]["tools"] = json!({"floor": floor, "ceiling": ceiling});
    research_binding(&value, &profiles())
}

#[test]
fn tool_derivation_never_widens_on_creator_loss() {
    let empty = BTreeSet::new();
    let sealed = strings(["read", "write"]);

    // Floor {}: the strict creator bound.
    let strict = tool_binding(&[], &["read", "write", "search"]);
    let live = strings(["read", "write"]);
    assert_eq!(
        effective_tools(&strict, &sealed, &empty, CreatorBound::Live(&live)),
        strings(["read", "write"])
    );
    let revoked = strings(["read"]);
    assert_eq!(
        effective_tools(&strict, &sealed, &empty, CreatorBound::Live(&revoked)),
        strings(["read"])
    );
    assert!(effective_tools(&strict, &sealed, &empty, CreatorBound::Unresolved).is_empty());
    assert_eq!(
        effective_tools(&strict, &sealed, &empty, CreatorBound::Live(&revoked)),
        strings(["read"]),
        "a recovered creator restores only its live bound, never the revoked write"
    );

    // Floor {write}: a constant host grant within the sealed request.
    let floored = tool_binding(&["write"], &["read", "write", "search"]);
    assert_eq!(
        effective_tools(&floored, &sealed, &empty, CreatorBound::Unresolved),
        strings(["write"])
    );
    assert_eq!(
        effective_tools(
            &floored,
            &strings(["read"]),
            &empty,
            CreatorBound::Unresolved
        ),
        BTreeSet::new(),
        "the floor grants only what the sealed request names"
    );

    // Approved excess: constant, from the human decision.
    let narrow = tool_binding(&[], &["read"]);
    let excess = strings(["shell"]);
    let sealed_with_excess = strings(["read", "shell"]);
    assert_eq!(
        effective_tools(
            &narrow,
            &sealed_with_excess,
            &excess,
            CreatorBound::Unresolved
        ),
        strings(["shell"])
    );
    assert_eq!(
        effective_tools(
            &narrow,
            &sealed_with_excess,
            &excess,
            CreatorBound::Unbounded
        ),
        strings(["read", "shell"])
    );
    assert_eq!(
        sealed_upper_bound(&narrow, &sealed_with_excess, &excess),
        strings(["read", "shell"])
    );
    assert_eq!(
        effective_tools(
            &narrow,
            &sealed_with_excess,
            &empty,
            CreatorBound::Unbounded
        ),
        strings(["read"]),
        "an excess no decision approved grants nothing"
    );
    let policy = native_tool_policy(&strings(["read", "shell"]));
    assert!(matches!(
        policy,
        meerkat_core::ToolAccessPolicy::AllowList(_)
    ));
}

#[test]
fn the_preamble_composes_creator_peers_display_name_and_purpose() {
    let preamble = Preamble::parse(
        "Created by {creator}. Peers: {peers}. You are {display_name}: {purpose}. \
         Literal {{braces}}.",
    )
    .expect("a preamble");
    let peers = ["peer-b".to_owned(), "peer-a".to_owned()];
    let inputs = PreambleInputs {
        creator: "lead-1",
        peers: &peers,
        display_name: "Helper",
        purpose: "Find sources",
    };
    assert_eq!(
        preamble.render(&inputs),
        "Created by lead-1. Peers: peer-a, peer-b. You are Helper: Find sources. \
         Literal {braces}."
    );
    let alone = PreambleInputs {
        peers: &[],
        ..inputs
    };
    assert!(preamble.render(&alone).contains("Peers: none."));
    assert_eq!(
        compose_member_instructions(&preamble, &inputs, "Do the work."),
        format!(
            "{}\n\n{CREATOR_INSTRUCTIONS_HEADING}\nDo the work.",
            preamble.render(&inputs)
        )
    );
    let none = Preamble::parse("").expect("an empty preamble");
    assert!(none.is_empty());
    assert_eq!(
        compose_member_instructions(&none, &inputs, "Do the work."),
        format!("{CREATOR_INSTRUCTIONS_HEADING}\nDo the work.")
    );
    assert_eq!(Preamble::parse("Hi {name}").unwrap_err(), "{name}");
    assert!(Preamble::parse("Hi {creator").is_err());
    assert!(Preamble::parse("Hi }").is_err());
    assert_eq!(
        RequesterFacts::console().requester.preamble_name(),
        "the console"
    );
    assert_eq!(lead().requester.preamble_name(), "lead-1");
}

// ---------------------------------------------------------------------------
// Normalization rows
// ---------------------------------------------------------------------------

#[test]
fn a_request_normalizes_with_its_binding_tools_and_plan() {
    let normalized = create(&policy(), &lead(), &[spec("helper")]).expect("normalized");
    let created = created_spec(&normalized.request, "team:helper");
    assert_eq!(created.profile, "assistant");
    assert_eq!(created.model, "claude-opus-5-5");
    assert_eq!(
        created.tools,
        strings(["notes_read", "search"]),
        "an agent's default tools are the ceiling it holds, and the floor"
    );
    assert_eq!(
        created.binding_digest,
        normalized.bindings["team:helper"].digest()
    );
    assert_eq!(
        normalized.plan.outcome,
        PlannedOutcome::Notify {
            approver_id: "owner".to_owned()
        }
    );
    assert_eq!(normalized.plan.timeout_ms, 86_400_000);
    assert!(!normalized.plan.raised);
    assert_eq!(normalized.request.decision_kind(), "identity.create");
    let host = create(
        &policy(),
        &RequesterFacts::host("gateway"),
        &[spec("helper")],
    )
    .expect("normalized");
    assert_eq!(
        created_spec(&host.request, "team:helper").tools,
        strings(["notes_read", "notes_write", "search"]),
        "the host's default tools are the ceiling"
    );
    assert_eq!(host.plan.outcome, PlannedOutcome::Allow);
}

type SpecChange = fn(&mut CreateIdentitySpec);

fn spec_case(
    change: SpecChange,
    kind: IdentityRefusalKind,
    field: &'static str,
) -> (SpecChange, IdentityRefusalKind, &'static str) {
    (change, kind, field)
}

#[test]
fn normalization_refuses_typed() {
    let cases = [
        spec_case(
            |spec| spec.slug = "Bad".to_owned(),
            IdentityRefusalKind::InvalidName,
            "identities[0].slug",
        ),
        spec_case(
            |spec| spec.slug = "x".to_owned(),
            IdentityRefusalKind::InvalidName,
            "identities[0].slug",
        ),
        spec_case(
            |spec| spec.slug = "admin".to_owned(),
            IdentityRefusalKind::ReservedName,
            "identities[0].slug",
        ),
        spec_case(
            |spec| spec.template = "nope".to_owned(),
            IdentityRefusalKind::TemplateNotAllowed {
                template: "nope".to_owned(),
            },
            "identities[0].template",
        ),
        spec_case(
            |spec| spec.profile = Some("lead".to_owned()),
            IdentityRefusalKind::ProfileNotAllowed {
                profile: Some("lead".to_owned()),
            },
            "identities[0].profile",
        ),
        spec_case(
            |spec| spec.model = Some("gpt-5.5".to_owned()),
            IdentityRefusalKind::ModelNotAllowed {
                model: "gpt-5.5".to_owned(),
            },
            "identities[0].model",
        ),
        spec_case(
            |spec| {
                spec.descriptive_labels = BTreeMap::from([("team".to_owned(), "x".to_owned())]);
            },
            IdentityRefusalKind::ReservedKey {
                key: "team".to_owned(),
            },
            "identities[0].descriptive_labels",
        ),
        spec_case(
            |spec| {
                spec.descriptive_labels = BTreeMap::from([("role".to_owned(), "x".to_owned())]);
            },
            IdentityRefusalKind::ReservedKey {
                key: "role".to_owned(),
            },
            "identities[0].descriptive_labels",
        ),
        spec_case(
            |spec| {
                spec.descriptive_labels = BTreeMap::from([("bad key".to_owned(), "x".to_owned())]);
            },
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            },
            "identities[0].descriptive_labels",
        ),
        spec_case(
            |spec| {
                spec.descriptive_labels = BTreeMap::from([(String::new(), "x".to_owned())]);
            },
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            },
            "identities[0].descriptive_labels",
        ),
        spec_case(
            |spec| {
                spec.descriptive_labels = BTreeMap::from([("note".to_owned(), "a\nb".to_owned())]);
            },
            IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            },
            "identities[0].descriptive_labels.note",
        ),
        spec_case(
            |spec| {
                spec.descriptive_labels = BTreeMap::from([("note".to_owned(), "x".repeat(257))]);
            },
            IdentityRefusalKind::TextTooLarge { limit: 256 },
            "identities[0].descriptive_labels.note",
        ),
        spec_case(
            |spec| {
                spec.supplied_context = Some(BTreeMap::from([("project".to_owned(), json!("x"))]));
            },
            IdentityRefusalKind::ReservedKey {
                key: "project".to_owned(),
            },
            "identities[0].supplied_context",
        ),
        spec_case(
            |spec| spec.display_name = "x".repeat(65),
            IdentityRefusalKind::TextTooLarge { limit: 64 },
            "identities[0].display_name",
        ),
        spec_case(
            |spec| spec.kickoff = Some("k".repeat(501)),
            IdentityRefusalKind::TextTooLarge { limit: 500 },
            "identities[0].kickoff",
        ),
        spec_case(
            |spec| {
                spec.authority_fields =
                    BTreeMap::from([("clearance".to_owned(), "top".to_owned())]);
            },
            IdentityRefusalKind::AuthorityFieldInvalid {
                key: "clearance".to_owned(),
            },
            "identities[0].authority_fields",
        ),
        spec_case(
            |spec| {
                spec.authority_fields = BTreeMap::from([("team".to_owned(), "core".to_owned())]);
            },
            IdentityRefusalKind::AuthorityFieldInvalid {
                key: "team".to_owned(),
            },
            "identities[0].authority_fields",
        ),
        spec_case(
            |spec| spec.peers = vec!["ghost".to_owned()],
            IdentityRefusalKind::PeerUnknown,
            "identities[0].peers[0]",
        ),
        spec_case(
            |spec| spec.peers = vec!["stranger".to_owned()],
            IdentityRefusalKind::PeerNotAllowed {
                identity: "stranger".to_owned(),
            },
            "identities[0].peers[0]",
        ),
        spec_case(
            |spec| spec.peers = vec!["team:helper".to_owned()],
            IdentityRefusalKind::PeerNotAllowed {
                identity: "team:helper".to_owned(),
            },
            "identities[0].peers[0]",
        ),
    ];
    for (change, kind, field) in cases {
        let mut request = spec("helper");
        change(&mut request);
        let refusal = create(&policy(), &lead(), &[request]).expect_err("refused");
        assert_eq!(refusal.kind, kind, "{refusal}");
        assert_eq!(refusal.field.as_deref(), Some(field), "{refusal}");
        assert!(!refusal.is_retryable());
    }

    let refusal =
        create(&policy(), &lead(), &[spec("helper"), spec("helper")]).expect_err("a duplicate");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::NameTaken {
            by: NameTakenBy::Created
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("identities[1].slug"));

    let refusal = create(&policy(), &lead(), &[]).expect_err("nothing to create");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty
        }
    );

    let envelope = RequestEnvelope {
        proposer: Some("an agent cannot name a proposer".to_owned()),
        ..RequestEnvelope::default()
    };
    let refusal =
        create_with(&policy(), &lead(), envelope, &[spec("helper")]).expect_err("agent envelope");
    assert_eq!(refusal.field.as_deref(), Some("proposer"));

    let human = RequesterFacts {
        requester: Requester::Human {
            principal: "principal:person".to_owned(),
        },
        ..RequesterFacts::console()
    };
    let refusal = create(&policy(), &human, &[spec("helper")]).expect_err("no rule");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::CreationDenied {
            change: ChangeClass::Create
        },
        "no governing rule gets the default, deny"
    );
}

#[test]
fn the_kickoff_rule_refuses_typed() {
    let mut value = policy_json();
    value["templates"]["research"]["kickoff"] = json!("forbidden");
    let forbidden = parse(&value);
    let mut with_kickoff = spec("helper");
    with_kickoff.kickoff = Some("Start now.".to_owned());
    let refusal = create(&forbidden, &lead(), &[with_kickoff]).expect_err("forbidden");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::KickoffRefused {
            rule: KickoffRule::Forbidden
        }
    );
    let mut empty_kickoff = spec("helper");
    empty_kickoff.kickoff = Some(String::new());
    assert!(
        create(&forbidden, &lead(), &[empty_kickoff.clone()]).is_ok(),
        "an empty kickoff is none"
    );

    value["templates"]["research"]["kickoff"] = json!("required");
    let required = parse(&value);
    let refusal = create(&required, &lead(), &[empty_kickoff]).expect_err("required");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::KickoffRefused {
            rule: KickoffRule::Required
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("identities[0].kickoff"));
    assert_eq!(refusal.to_data()["rule"], "required");
}

#[test]
fn strict_mode_refuses_excess() {
    let policy = policy();
    let mut outside = spec("helper");
    outside.tools = Some(vec!["search".to_owned(), "shell".to_owned()]);
    let refusal = create(&policy, &lead(), &[outside.clone()]).expect_err("exceeds the template");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::ExceedsTemplate {
            tools: vec!["shell".to_owned()]
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("identities[0].tools"));
    assert_eq!(refusal.to_data()["tools"], json!(["shell"]));
    assert!(
        create(&policy, &RequesterFacts::host("gateway"), &[outside]).is_err(),
        "the ceiling bounds the host too"
    );

    let mut not_held = spec("helper");
    not_held.tools = Some(vec!["notes_write".to_owned()]);
    let refusal = create(&policy, &lead(), &[not_held.clone()]).expect_err("exceeds the creator");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::ExceedsCreator {
            tools: vec!["notes_write".to_owned()]
        }
    );
    assert!(
        create(&policy, &RequesterFacts::host("gateway"), &[not_held]).is_ok(),
        "the host has no creator bound"
    );

    let mut floor_only = spec("helper");
    floor_only.tools = Some(vec!["notes_read".to_owned()]);
    let lacking_floor = RequesterFacts::agent(
        "lead-1",
        Some("lead".to_owned()),
        BTreeMap::new(),
        strings(["search"]),
    );
    assert!(
        create(&policy, &lacking_floor, &[floor_only]).is_ok(),
        "a floor tool is never an excess over the creator"
    );
}

#[test]
fn consequence_mode_turns_excess_into_require_approval_with_lines() {
    let mut value = policy_json();
    value["templates"]["research"]["excess"] = json!("consequence");
    let policy = parse(&value);
    let mut request = spec("helper");
    request.tools = Some(vec![
        "notes_read".to_owned(),
        "notes_write".to_owned(),
        "shell".to_owned(),
    ]);
    let normalized = create(&policy, &lead(), &[request]).expect("consequence mode admits");
    assert!(normalized.plan.raised);
    assert_eq!(
        normalized.plan.outcome,
        PlannedOutcome::RequireApproval {
            approver_id: "owner".to_owned()
        },
        "the notify rule's approver decides the raised request"
    );
    let effects: Vec<&ConsequenceEffect> = normalized
        .request
        .consequences
        .iter()
        .map(|consequence| &consequence.effect)
        .collect();
    assert_eq!(
        effects,
        [
            &ConsequenceEffect::ToolOutsideTemplate {
                tool: "shell".to_owned()
            },
            &ConsequenceEffect::ToolNotHeldByCreator {
                tool: "notes_write".to_owned()
            },
            &ConsequenceEffect::ToolNotHeldByCreator {
                tool: "shell".to_owned()
            },
        ]
    );
    assert_eq!(
        normalized.request.excess_tools("team:helper"),
        strings(["notes_write", "shell"])
    );
    let rendering = render(&normalized.request);
    for line in [
        "- \"team:helper\" tool \"shell\" is outside the template ceiling\n",
        "- \"team:helper\" tool \"notes\\_write\" is not held by the creator\n",
        "- \"team:helper\" tool \"shell\" is not held by the creator\n",
    ] {
        assert!(rendering.as_str().contains(line), "{line}");
    }
    let card = rendering.card().expect("a card");
    let consequences = card
        .blocks()
        .iter()
        .flat_map(CardBlock::entries)
        .find_map(|entry| match entry {
            CardEntry::List { label, items } if label == labels::CONSEQUENCES => Some(items),
            _ => None,
        })
        .expect("the consequences");
    assert_eq!(consequences.len(), 3);

    // A template open to every profile forces a human decision for create.
    let mut open = spec("helper");
    open.template = "open".to_owned();
    open.profile = Some("assistant".to_owned());
    let normalized = create(&policy, &lead(), &[open.clone()]).expect("any profile");
    assert!(normalized.plan.raised);
    assert!(normalized.request.consequences.contains(&Consequence {
        identity: "open:helper".to_owned(),
        effect: ConsequenceEffect::AnyProfile {
            profile: "assistant".to_owned()
        },
    }));
    // The host's create rule is allow, which names no approver: a raised
    // request has nobody to decide and is refused before anything is held.
    let refusal =
        create(&policy, &RequesterFacts::host("gateway"), &[open]).expect_err("nobody can decide");
    assert_eq!(refusal.kind, IdentityRefusalKind::ApprovalUnavailable);
}

#[test]
fn consequence_mode_refuses_tools_no_decision_can_grant() {
    let mut value = policy_json();
    value["templates"]["research"]["excess"] = json!("consequence");
    let policy = parse(&value);
    for tool in ["identity_create", "shell\u{202E}", ""] {
        let mut request = spec("helper");
        request.tools = Some(vec!["search".to_owned(), tool.to_owned()]);
        let refusal = create(&policy, &lead(), &[request]).expect_err("no decision can grant it");
        assert_eq!(
            refusal.kind,
            IdentityRefusalKind::ExceedsTemplate {
                tools: vec![tool.to_owned()]
            },
            "{tool:?}"
        );
        assert_eq!(refusal.field.as_deref(), Some("identities[0].tools"));
    }

    // A template that grants creation lets a human decide on a creation
    // tool beyond its ceiling.
    value["templates"]["research"]["may_grant_creation"] = json!(true);
    let granting = parse(&value);
    let mut request = spec("helper");
    request.tools = Some(vec!["identity_create".to_owned()]);
    let normalized = create(&granting, &lead(), &[request]).expect("a human decides");
    assert!(normalized.plan.raised);
    assert!(normalized.request.consequences.contains(&Consequence {
        identity: "team:helper".to_owned(),
        effect: ConsequenceEffect::ToolOutsideTemplate {
            tool: "identity_create".to_owned()
        },
    }));
}

#[test]
fn peer_consequence_lines_say_why_a_peer_needs_approval() {
    let mut request = spec("helper");
    request.peers = vec![
        "lead-1".to_owned(),
        "core-1".to_owned(),
        "analyst-1".to_owned(),
        "lead-1".to_owned(),
    ];
    let normalized = create(&policy(), &lead(), &[request]).expect("normalized");
    let created = created_spec(&normalized.request, "team:helper");
    assert_eq!(
        created.peers,
        BTreeMap::from([
            ("analyst-1".to_owned(), EdgeBasis::CreatorPeer),
            (
                "core-1".to_owned(),
                EdgeBasis::Rule {
                    id: "sensitive".to_owned()
                }
            ),
            ("lead-1".to_owned(), EdgeBasis::Creator),
        ])
    );
    assert!(normalized.plan.raised);
    assert_eq!(normalized.plan.kind(), ApprovalKind::RequireApproval);
    let rendering = render(&normalized.request);
    assert!(rendering.as_str().contains(
        "- \"team:helper\" peer \"core-1\" needs approval under rule \"sensitive\" because \
         \"a core team member\"\n"
    ));

    let verdict = evaluate_peer(
        policy().peers(),
        "stranger",
        &peers()["stranger"],
        &BTreeSet::new(),
    );
    assert_eq!(
        verdict,
        PeerVerdict::Denied {
            rule: None,
            reason: None
        },
        "the default denies, and the directory can say so"
    );
}

#[test]
fn authority_fields_need_a_human_decision() {
    let mut request = spec("helper");
    request.authority_fields = BTreeMap::from([("clearance".to_owned(), "elevated".to_owned())]);
    let refusal =
        create(&policy(), &lead(), &[request.clone()]).expect_err("notify is no human decision");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::AuthorityFieldNeedsApproval {
            key: "clearance".to_owned()
        }
    );

    let mut value = policy_json();
    value["approval"]["rules"][0]["outcome"] =
        json!({"kind": "require_approval", "approver": {"id": "owner"}});
    let normalized = create(&parse(&value), &lead(), &[request]).expect("a human decides");
    assert_eq!(normalized.plan.kind(), ApprovalKind::RequireApproval);
    assert!(normalized.request.consequences.contains(&Consequence {
        identity: "team:helper".to_owned(),
        effect: ConsequenceEffect::AuthorityField {
            key: "clearance".to_owned(),
            value: "elevated".to_owned()
        },
    }));
    assert!(render(&normalized.request).as_str().contains(
        "- \"team:helper\" authority field \"clearance\" = \"elevated\" becomes authority\n"
    ));
}

#[test]
fn the_request_digest_is_stable_across_input_order() {
    let mut first = spec("helper");
    first.tools = Some(vec!["search".to_owned(), "notes_read".to_owned()]);
    first.peers = vec!["lead-1".to_owned(), "analyst-1".to_owned()];
    let mut second = spec("scout");
    second.peers = vec!["team:helper".to_owned()];
    let forward = create(&policy(), &lead(), &[first.clone(), second.clone()]).expect("forward");
    first.tools = Some(vec![
        "notes_read".to_owned(),
        "search".to_owned(),
        "search".to_owned(),
    ]);
    first.peers = vec!["analyst-1".to_owned(), "lead-1".to_owned()];
    let backward = create(&policy(), &lead(), &[second, first]).expect("backward");
    assert_eq!(forward.request, backward.request);
    assert_eq!(
        forward.request.request_digest().expect("a digest"),
        backward.request.request_digest().expect("a digest")
    );
    assert_eq!(render(&forward.request), render(&backward.request));
    assert_eq!(
        created_spec(&forward.request, "team:scout").peers["team:helper"],
        EdgeBasis::SameBatch
    );
    let json = forward.request.to_json().expect("json");
    let read_back: IdentityRequest = serde_json::from_value(json).expect("the body reads back");
    assert_eq!(read_back, forward.request);
}

fn entry_facts(binding_digest: Sha256Digest) -> CreatedEntryFacts {
    CreatedEntryFacts {
        identity: "team:helper".to_owned(),
        revision: 3,
        template: "research".to_owned(),
        binding_digest,
        profile: "assistant".to_owned(),
        model: "claude-opus-5-5".to_owned(),
        creator: lead().requester,
        creator_live_tools: Some(lead().live_tools),
    }
}

fn current_binding() -> Sha256Digest {
    research_binding(&policy_json(), &profiles()).digest()
}

fn update(
    requester: &RequesterFacts,
    entry: &CreatedEntryFacts,
    spec: &UpdateIdentitySpec,
) -> Result<NormalizedRequest, IdentityRefusal> {
    update_with(&policy(), requester, entry, spec)
}

/// An update under `policy`.
fn update_with(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    entry: &CreatedEntryFacts,
    spec: &UpdateIdentitySpec,
) -> Result<NormalizedRequest, IdentityRefusal> {
    let profiles = profiles();
    let peers = peers();
    let context = RequestContext {
        requester,
        profiles: &profiles,
        peers: &peers,
    };
    normalize_update(policy, &context, RequestEnvelope::default(), entry, spec)
}

fn update_spec(set: UpdateSetSpec) -> UpdateIdentitySpec {
    UpdateIdentitySpec {
        identity: "team:helper".to_owned(),
        expected_revision: 3,
        set,
        rebind: false,
    }
}

#[test]
fn a_host_update_checks_tools_against_the_creators_live_tools() {
    let entry = entry_facts(current_binding());
    let host = RequesterFacts::host("gateway");
    let tools = |tools: &[&str]| {
        update_spec(UpdateSetSpec {
            tools: Some(tools.iter().map(|tool| (*tool).to_owned()).collect()),
            ..UpdateSetSpec::default()
        })
    };
    // notes_write is in the ceiling but not held by the agent creator: the
    // host's update cannot seal it past the creator's bound.
    assert_eq!(
        update(&host, &entry, &tools(&["notes_write"]))
            .unwrap_err()
            .kind,
        IdentityRefusalKind::ExceedsCreator {
            tools: vec!["notes_write".to_owned()]
        }
    );
    assert!(update(&host, &entry, &tools(&["search"])).is_ok());

    // An unresolved creator lets nothing through but the floor.
    let unresolved = CreatedEntryFacts {
        creator_live_tools: None,
        ..entry
    };
    assert_eq!(
        update(&host, &unresolved, &tools(&["search"]))
            .unwrap_err()
            .kind,
        IdentityRefusalKind::ExceedsCreator {
            tools: vec!["search".to_owned()]
        }
    );
    assert!(update(&host, &unresolved, &tools(&["notes_read"])).is_ok());
}

#[test]
fn update_and_remove_check_custody_revision_and_binding() {
    let entry = entry_facts(current_binding());
    let purpose = update_spec(UpdateSetSpec {
        purpose: Some("Find better sources.".to_owned()),
        ..UpdateSetSpec::default()
    });
    let normalized = update(&lead(), &entry, &purpose).expect("the creator updates");
    assert_eq!(normalized.plan.change, ChangeClass::Descriptive);
    assert_eq!(normalized.plan.kind(), ApprovalKind::Notify);
    assert_eq!(normalized.request.decision_kind(), "identity.update");

    let other = RequesterFacts::agent("other-1", None, BTreeMap::new(), BTreeSet::new());
    assert_eq!(
        update(&other, &entry, &purpose).unwrap_err().kind,
        IdentityRefusalKind::NotCreatorCustody
    );
    let itself = RequesterFacts::agent("team:helper", None, BTreeMap::new(), BTreeSet::new());
    assert_eq!(
        update(&itself, &entry, &purpose).unwrap_err().kind,
        IdentityRefusalKind::SelfChangeRefused
    );
    let stale = UpdateIdentitySpec {
        expected_revision: 2,
        ..purpose.clone()
    };
    let refusal = update(&lead(), &entry, &stale).unwrap_err();
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::StaleRevision { current: 3 }
    );
    assert_eq!(refusal.to_data()["current"], 3);
    assert_eq!(
        update(&lead(), &entry, &update_spec(UpdateSetSpec::default()))
            .unwrap_err()
            .kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty
        }
    );
    let conflicting = update_spec(UpdateSetSpec {
        peers_add: vec!["core-1".to_owned()],
        peers_remove: vec!["core-1".to_owned()],
        ..UpdateSetSpec::default()
    });
    assert_eq!(
        update(&lead(), &entry, &conflicting).unwrap_err().kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Conflicting
        }
    );

    let tools = update_spec(UpdateSetSpec {
        tools: Some(vec!["search".to_owned()]),
        peers_add: vec!["analyst-1".to_owned()],
        ..UpdateSetSpec::default()
    });
    let normalized = update(&lead(), &entry, &tools).expect("capability and peers");
    assert_eq!(normalized.plan.kind(), ApprovalKind::RequireApproval);
    let RequestAction::Update(updated) = &normalized.request.action else {
        panic!("an update");
    };
    assert_eq!(
        updated.change_classes(),
        BTreeSet::from([ChangeClass::Capability, ChangeClass::Peers])
    );
    assert_eq!(updated.peers_add["analyst-1"], EdgeBasis::CreatorPeer);
    let excess = update_spec(UpdateSetSpec {
        tools: Some(vec!["notes_write".to_owned()]),
        ..UpdateSetSpec::default()
    });
    assert_eq!(
        update(&lead(), &entry, &excess).unwrap_err().kind,
        IdentityRefusalKind::ExceedsCreator {
            tools: vec!["notes_write".to_owned()]
        }
    );

    let rebind = UpdateIdentitySpec {
        rebind: true,
        ..purpose.clone()
    };
    assert_eq!(
        update(&lead(), &entry, &rebind).unwrap_err().kind,
        IdentityRefusalKind::CreationDenied {
            change: ChangeClass::Rebind
        }
    );
    let moved = entry_facts(Sha256Digest::of(b"an older binding"));
    assert_eq!(
        update(&lead(), &moved, &purpose).unwrap_err().kind,
        IdentityRefusalKind::TemplateBindingChanged
    );
    let host_rebind = UpdateIdentitySpec {
        rebind: true,
        ..update_spec(UpdateSetSpec::default())
    };
    let normalized =
        update(&RequesterFacts::host("gateway"), &moved, &host_rebind).expect("the host rebinds");
    let RequestAction::Update(updated) = &normalized.request.action else {
        panic!("an update");
    };
    assert_eq!(updated.rebind, Some(current_binding()));
    assert_eq!(normalized.plan.kind(), ApprovalKind::Allow);
    assert_eq!(
        update(&RequesterFacts::host("gateway"), &entry, &host_rebind)
            .unwrap_err()
            .kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty
        },
        "a rebind to the same binding changes nothing"
    );

    let remove = RemoveIdentitySpec {
        identity: "team:helper".to_owned(),
        expected_revision: 3,
    };
    let removed = normalize_remove(
        &policy(),
        &lead(),
        RequestEnvelope::default(),
        &entry,
        &remove,
    )
    .expect("the creator removes");
    assert_eq!(removed.plan.kind(), ApprovalKind::Notify);
    assert_eq!(removed.request.decision_kind(), "identity.remove");
    assert_eq!(
        normalize_remove(
            &policy(),
            &itself,
            RequestEnvelope::default(),
            &entry,
            &remove
        )
        .unwrap_err()
        .kind,
        IdentityRefusalKind::SelfChangeRefused
    );
}

// ---------------------------------------------------------------------------
// render_v1: the canonical text
// ---------------------------------------------------------------------------

const GOLDEN_CREATE: &str = r#"Identity request: render-v1
Action: create
Requester: agent "lead-1"
Proposer: none
Source evidence: none
Identities: 1

Identity 1 of 1: "team:helper"
Template: "research"
Template binding: @BINDING@
Profile: "assistant"
Model: "claude-opus-5-5"
Tools (2):
- "notes\_read"
- "search"
Peers (2):
- "core-1" basis rule "sensitive"
- "lead-1" basis creator
Authority fields (1):
- "clearance" = "elevated"
Descriptive labels (1):
- "color" = "blue"

Consequences (2):
- "team:helper" peer "core-1" needs approval under rule "sensitive" because "a core team member"
- "team:helper" authority field "clearance" = "elevated" becomes authority

Host consequences (0):

Text of identity 1 of 1: "team:helper"
Display name (1 line):
> Helper
Purpose (1 line):
> Find sources.
Instructions (2 lines):
> Read the notes.
> Report daily.
Kickoff: none
Supplied context (4 lines):
> {
>   "depth": 2,
>   "topic": "stars"
> }
"#;

const GOLDEN_UPDATE: &str = r#"Identity request: render-v1
Action: update
Requester: agent "lead-1"
Proposer: none
Source evidence: none
Identity: "team:helper"
Expected revision: 3
Template: "research"
Template binding: @BINDING@
Template rebind: none
Changes: descriptive capability peers

Update of: "team:helper"
Tools (1):
- "search"
Peers added (1):
- "analyst-1" basis creator-peer
Peers removed (1):
- "core-1"
Authority fields: unchanged
Descriptive labels: unchanged

Consequences (0):

Host consequences (0):

Text of: "team:helper"
Display name: unchanged
Purpose (1 line):
> Find better sources.
Instructions: unchanged
Supplied context: unchanged
"#;

const GOLDEN_REMOVE: &str = r#"Identity request: render-v1
Action: remove
Requester: host "console"
Proposer: "op-proposal-7"
Source attestor: "adapter-1"
Source subject: "user-9"
Source reference: "event:42"
Source observed at ms: 1730000000000
Identity: "team:helper"
Expected revision: 3

Consequences (0):

Host consequences (1):
- "Removes a helper that holds private notes"
"#;

#[test]
fn render_v1_is_the_pinned_canonical_text() {
    let binding = binding_digest();
    assert_eq!(
        render(&golden_create()).as_str(),
        GOLDEN_CREATE.replace("@BINDING@", binding.as_str())
    );
    assert_eq!(
        render(&golden_update()).as_str(),
        GOLDEN_UPDATE.replace("@BINDING@", binding.as_str())
    );
    let hook = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["Removes a helper that holds private notes"]))
    };
    let removal = render_v1(&golden_remove(), Some(&hook)).expect("a rendering");
    assert_eq!(removal.as_str(), GOLDEN_REMOVE);
    assert_eq!(
        removal.subject_digest(),
        Sha256Digest::of(GOLDEN_REMOVE.as_bytes())
    );
    for golden in [GOLDEN_CREATE, GOLDEN_UPDATE, GOLDEN_REMOVE] {
        assert!(StructuredCard::parse(&golden.replace("@BINDING@", binding.as_str())).is_ok());
    }
}

#[test]
fn render_v1_is_deterministic() {
    let first = render(&golden_create());
    let second = render(&golden_create());
    assert_eq!(first, second);
    assert_eq!(first.subject_digest(), second.subject_digest());
    assert!(first.as_str().ends_with('\n'));
    assert!(!first.as_str().contains("\n\n\n"));
}

#[test]
fn escaping_round_trips_and_neutralizes_markup() {
    let samples = [
        "plain words",
        "back\\slash",
        "a \"quoted\" word",
        "||spoiler||",
        "[link](https://example.invalid)",
        "<details><summary>x</summary>hidden</details>",
        "`code` *bold* _it_ ~strike~ R&D",
        "rtl\u{202E}override",
        "zero\u{200B}width join\u{200D}",
        "tab\tcarriage\rline\u{2028}",
        "nbsp\u{00A0}space",
        "tag\u{E0041}smuggled",
        "heart \u{2764}\u{FE0F}",
        "private\u{E000}use",
        "nonchar\u{FFFF}",
        "accents é ü and 漢字",
        "",
    ];
    for sample in samples {
        let line = escaped_line(sample);
        assert_eq!(unescape_line(&line).as_deref(), Some(sample), "{line}");
        assert!(!line.chars().any(is_hidden), "{line}");
        let mut escaped = false;
        for character in line.chars() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else {
                assert!(!is_markup(character), "raw markup in {line}");
            }
        }
        let value = quoted(sample);
        let (decoded, rest) = unquote(&value).expect("a quoted value");
        assert_eq!(decoded, sample);
        assert!(rest.is_empty());
    }
    assert_eq!(escaped_line("a_b"), "a\\_b");
    assert_eq!(escaped_line("x\u{202E}y"), "x\\u{202E}y");
    assert_eq!(quoted("say \"hi\""), "\"say \\\"hi\\\"\"");
    for refused in [
        "a_b",
        "\\u{41}",
        "\\u{a}",
        "\\u{000a}",
        "\\q",
        "x\u{202E}",
        "\\",
    ] {
        assert_eq!(unescape_line(refused), None, "{refused:?}");
    }
    assert_eq!(unquote("\"open"), None);
    assert_eq!(
        unquote("\"raw \" quote\""),
        Some(("raw ".to_owned(), " quote\""))
    );
}

#[test]
fn a_rendering_past_its_limit_is_refused() {
    let mut request = golden_create();
    created_spec_mut(&mut request, "team:helper").instructions = "x".repeat(RENDERING_MAX_BYTES);
    let refusal = render_v1(&request, None).expect_err("too large");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TextTooLarge {
            limit: RENDERING_MAX_BYTES as u64
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("rendering"));
}

#[test]
fn a_request_body_past_its_limit_is_refused() {
    let mut value = policy_json();
    value["templates"]["research"]["max_bytes"]["instructions"] = json!(200_000);
    let policy = parse(&value);
    // 140000 quotes: about 140 KB of rendering, but about 280 KB of JSON.
    let mut quotes = spec("helper");
    quotes.instructions = "\"".repeat(140_000);
    let refusal = create(&policy, &lead(), &[quotes.clone()]).expect_err("a body past its limit");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TextTooLarge {
            limit: REQUEST_BODY_MAX_BYTES as u64
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("request"));
    // The same length of plain text fits: the body was the bound, not the
    // text.
    quotes.instructions = "x".repeat(140_000);
    let fits = create(&policy, &lead(), &[quotes]).expect("a body within its limit");
    assert!(render_v1(&fits.request, None).is_ok());
}

// ---------------------------------------------------------------------------
// The contract's rendering rows, as pure render and digest checks
// ---------------------------------------------------------------------------

#[test]
fn an_escaped_rendering_of_an_unchanged_request_approves() {
    let mut request = golden_create();
    {
        let spec = created_spec_mut(&mut request, "team:helper");
        spec.display_name = "||Approve me|| [now](https://example.invalid)".to_owned();
        spec.purpose = "<b>bold</b> & *loud* ~gone~ _quiet_".to_owned();
        spec.instructions = "line one\u{202E}reversed\nzero\u{200B}width\n```code```".to_owned();
    }
    let rendering = render(&request);
    let text = rendering.as_str();
    for escaped in [
        "\\|\\|Approve me\\|\\| \\[now\\](https://example.invalid)",
        "\\<b\\>bold\\</b\\> \\& \\*loud\\* \\~gone\\~ \\_quiet\\_",
        "line one\\u{202E}reversed",
        "zero\\u{200B}width",
        "\\`\\`\\`code\\`\\`\\`",
    ] {
        assert!(text.contains(escaped), "{escaped}");
    }
    assert!(!text.contains("||Approve me||"));
    assert!(
        !text
            .chars()
            .any(|character| character != '\n' && is_hidden(character))
    );

    // The card shows the escaped text, and decodes, for comparison only,
    // the exact text the request carries.
    let card = rendering.card().expect("a card");
    let texts: BTreeMap<&str, &CardEntry> = card
        .blocks()
        .iter()
        .flat_map(CardBlock::entries)
        .filter(|entry| matches!(entry, CardEntry::Text { .. }))
        .map(|entry| (entry.label(), entry))
        .collect();
    let spec = created_spec(&request, "team:helper");
    assert_eq!(
        texts[labels::DISPLAY_NAME].display_lines(),
        [
            "Display name (1 line):",
            "> \\|\\|Approve me\\|\\| \\[now\\](https://example.invalid)"
        ]
    );
    for (label, expected) in [
        (labels::DISPLAY_NAME, &spec.display_name),
        (labels::PURPOSE, &spec.purpose),
        (labels::INSTRUCTIONS, &spec.instructions),
    ] {
        assert_eq!(
            texts[label].decoded_text_not_for_display().as_ref(),
            Some(expected),
            "{label}"
        );
    }

    // A presenter escapes again for its channel. That transform is never a
    // digest input: the decide carries the subject digest of the canonical
    // text it was given, and the presentation records the canonical parts.
    let channel_text = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    assert_ne!(channel_text, text);
    assert_ne!(
        Sha256Digest::of(channel_text.as_bytes()),
        rendering.subject_digest()
    );
    let parts = rendering.canonical_parts(None);
    assert_eq!(
        decide(&rendering, &rendering.subject_digest(), None, &parts),
        Ok(())
    );
    let unchanged = render(&request);
    assert_eq!(
        decide(&unchanged, &rendering.subject_digest(), None, &parts),
        Ok(()),
        "the same request renders the same subject"
    );
}

fn long_request() -> IdentityRequest {
    let mut request = golden_create();
    let spec = created_spec_mut(&mut request, "team:helper");
    spec.instructions = (0..120)
        .map(|line| format!("Step {line}: read the café notes ✓ and write 漢字 summaries."))
        .collect::<Vec<_>>()
        .join("\n");
    request
}

#[test]
fn a_complete_multi_part_presentation_approves() {
    let rendering = render(&long_request());
    let text = rendering.as_str();
    assert!(text.len() > 4096);
    for size in [1, 3, 16, 64, 1000, 4096] {
        let max = part_size(size);
        let ranges = part_ranges(text, max);
        let parts = rendering.canonical_parts(max);
        assert!(parts.len() > 1, "{size}");
        assert_eq!(ranges.len(), parts.len());
        let mut joined = String::new();
        for (index, (range, part)) in ranges.iter().zip(&parts).enumerate() {
            let bytes = text.get(range.clone()).expect("a char boundary");
            assert!(bytes.len() <= size as usize || bytes.chars().count() == 1);
            assert_eq!(part.index as usize, index);
            assert_eq!(part.count as usize, parts.len());
            assert_eq!(part.part_digest, Sha256Digest::of(bytes.as_bytes()));
            joined.push_str(bytes);
        }
        assert_eq!(joined, text, "the parts concatenate to the rendering");
        assert_eq!(
            decide(&rendering, &rendering.subject_digest(), max, &parts),
            Ok(()),
            "{size}"
        );
    }
    assert_eq!(rendering.canonical_parts(None).len(), 1);
    assert_eq!(rendering.canonical_parts(part_size(1_000_000)).len(), 1);
}

#[test]
fn a_changed_semantic_field_cannot_be_approved_by_an_old_tap() {
    let mut base = golden_create();
    base.requester = Requester::Host {
        surface: "gateway".to_owned(),
    };
    base.proposer = Some("proposal-1".to_owned());
    base.source_evidence = Some(SourceEvidence::new("adapter-1", "user-9", "event:42", 7));
    created_spec_mut(&mut base, "team:helper").kickoff = Some("Start.".to_owned());
    let old = render(&base);
    let old_tap = old.subject_digest();

    type RequestChange = fn(&mut IdentityRequest);
    fn change(change: RequestChange) -> RequestChange {
        change
    }
    fn spec_of(request: &mut IdentityRequest) -> &mut CreatedSpec {
        created_spec_mut(request, "team:helper")
    }
    fn evidence_of(request: &mut IdentityRequest) -> &mut SourceEvidence {
        request.source_evidence.as_mut().expect("source evidence")
    }
    let changes = [
        change(|request| {
            request.requester = Requester::Host {
                surface: "console".to_owned(),
            };
        }),
        change(|request| {
            request.requester = Requester::Agent {
                identity: "gateway".to_owned(),
            };
        }),
        change(|request| request.proposer = None),
        change(|request| request.proposer = Some("proposal-2".to_owned())),
        change(|request| request.source_evidence = None),
        change(|request| evidence_of(request).attestor = "adapter-2".to_owned()),
        change(|request| evidence_of(request).subject = "user-8".to_owned()),
        change(|request| evidence_of(request).source_ref = "event:43".to_owned()),
        change(|request| evidence_of(request).observed_at_ms = 8),
        change(|request| {
            let RequestAction::Create { identities } = &mut request.action else {
                panic!("a create request");
            };
            let spec = identities.remove("team:helper").expect("the identity");
            identities.insert("team:helper2".to_owned(), spec);
        }),
        change(|request| spec_of(request).template = "open".to_owned()),
        change(|request| spec_of(request).binding_digest = Sha256Digest::of(b"another binding")),
        change(|request| spec_of(request).profile = "analyst".to_owned()),
        change(|request| spec_of(request).model = "gpt-5.5".to_owned()),
        change(|request| spec_of(request).display_name = "Helpers".to_owned()),
        change(|request| spec_of(request).purpose = "Find sources!".to_owned()),
        change(|request| {
            spec_of(request).instructions = "Read the notes.\nReport daily.\n".to_owned();
        }),
        change(|request| {
            spec_of(request).instructions = "Read the notes. Report daily.".to_owned();
        }),
        change(|request| spec_of(request).supplied_context = None),
        change(|request| {
            spec_of(request).supplied_context = Some(BTreeMap::from([
                ("topic".to_owned(), json!("stars")),
                ("depth".to_owned(), json!(3)),
            ]));
        }),
        change(|request| {
            spec_of(request).supplied_context = Some(BTreeMap::from([
                ("topic".to_owned(), json!("stars")),
                ("depth".to_owned(), json!("2")),
            ]));
        }),
        change(|request| {
            spec_of(request)
                .descriptive_labels
                .insert("color".to_owned(), "red".to_owned());
        }),
        change(|request| {
            spec_of(request)
                .descriptive_labels
                .insert("shape".to_owned(), "round".to_owned());
        }),
        change(|request| {
            spec_of(request)
                .authority_fields
                .insert("clearance".to_owned(), "basic".to_owned());
        }),
        change(|request| {
            spec_of(request).tools.insert("notes_write".to_owned());
        }),
        change(|request| {
            spec_of(request).tools.remove("search");
        }),
        change(|request| {
            spec_of(request)
                .peers
                .insert("analyst-1".to_owned(), EdgeBasis::CreatorPeer);
        }),
        change(|request| {
            spec_of(request)
                .peers
                .insert("lead-1".to_owned(), EdgeBasis::SameBatch);
        }),
        change(|request| spec_of(request).kickoff = None),
        change(|request| spec_of(request).kickoff = Some(String::new())),
        change(|request| spec_of(request).kickoff = Some("Start!".to_owned())),
        change(|request| {
            request.consequences.insert(Consequence {
                identity: "team:helper".to_owned(),
                effect: ConsequenceEffect::ToolOutsideTemplate {
                    tool: "shell".to_owned(),
                },
            });
        }),
        change(|request| request.consequences.clear()),
    ];
    let mut digests = BTreeSet::from([old_tap.clone()]);
    for (index, apply) in changes.into_iter().enumerate() {
        let mut changed = base.clone();
        apply(&mut changed);
        assert_ne!(changed, base, "change {index} changes the request");
        let rendering = render(&changed);
        let parts = rendering.canonical_parts(part_size(256));
        assert_eq!(
            decide(&rendering, &old_tap, part_size(256), &parts),
            Err(IdentityRefusalKind::SubjectChanged),
            "change {index}"
        );
        assert!(
            digests.insert(rendering.subject_digest()),
            "change {index} renders a subject of its own"
        );
        assert_ne!(
            changed.request_digest().expect("a digest"),
            base.request_digest().expect("a digest")
        );
    }
}

#[test]
fn a_presentation_missing_a_part_cannot_approve() {
    let rendering = render(&long_request());
    let max = part_size(512);
    let parts = rendering.canonical_parts(max);
    assert!(parts.len() > 2);
    for missing in 0..parts.len() {
        let mut shown = parts.clone();
        shown.remove(missing);
        assert_eq!(
            decide(&rendering, &rendering.subject_digest(), max, &shown),
            Err(IdentityRefusalKind::PresentationIncomplete),
            "part {missing}"
        );
    }
    let mut reordered = parts;
    reordered.swap(0, 1);
    assert_eq!(
        decide(&rendering, &rendering.subject_digest(), max, &reordered),
        Err(IdentityRefusalKind::PresentationIncomplete)
    );
    let other_size = rendering.canonical_parts(part_size(256));
    assert_eq!(
        decide(&rendering, &rendering.subject_digest(), max, &other_size),
        Err(IdentityRefusalKind::PresentationIncomplete),
        "parts of another size are not this split"
    );
    let card = rendering.card().expect("a card");
    let some_blocks: Vec<usize> = (1..card.blocks().len()).collect();
    assert_eq!(
        decide(
            &rendering,
            &rendering.subject_digest(),
            max,
            &card.covered_parts_of(&some_blocks, max)
        ),
        Err(IdentityRefusalKind::PresentationIncomplete),
        "a card without its header block covers less than every part"
    );
}

// ---------------------------------------------------------------------------
// The structured card
// ---------------------------------------------------------------------------

#[test]
fn the_structured_card_is_parsed_from_the_canonical_rendering_and_covers_every_part() {
    let mut request = golden_create();
    let mut scout = golden_spec();
    scout.display_name = "Scout *one*".to_owned();
    scout.instructions = "Look around.\n\nReport [findings].".to_owned();
    scout.kickoff = Some("Begin.".to_owned());
    scout.supplied_context = None;
    let RequestAction::Create { identities } = &mut request.action else {
        panic!("a create request");
    };
    identities.insert("team:scout".to_owned(), scout.clone());
    let hook = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&[
            "Reads private notes",
            "Can message <the core team>",
        ]))
    };
    let rendering = render_v1(&request, Some(&hook)).expect("a rendering");
    let card = rendering.card().expect("a card");
    let blocks = card.blocks();
    assert_eq!(
        blocks.len(),
        7,
        "header, two identities, consequences, host lines, two texts"
    );

    // The blocks tile the rendering: every byte belongs to exactly one, and
    // a block's display lines are exactly its bytes.
    assert_eq!(blocks[0].bytes().start, 0);
    for pair in blocks.windows(2) {
        assert_eq!(pair[0].bytes().end, pair[1].bytes().start);
    }
    assert_eq!(blocks[6].bytes().end, rendering.as_str().len());
    for (index, block) in blocks.iter().enumerate() {
        let mut displayed = block.display_lines().join("\n");
        displayed.push('\n');
        if index + 1 < blocks.len() {
            displayed.push('\n');
        }
        assert_eq!(
            displayed,
            rendering.as_str()[block.bytes()],
            "block {index}"
        );
    }

    // Labelled fields, as the canonical text writes them.
    match blocks[0].heading() {
        Some(CardEntry::Field { label, value }) => {
            assert_eq!(label, labels::TITLE);
            assert_eq!(value.word(), Some(labels::VERSION));
        }
        other => panic!("{other:?}"),
    }
    match blocks[2].heading() {
        Some(CardEntry::Field { label, value }) => {
            assert_eq!(label, "Identity 2 of 2");
            assert_eq!(value.quoted(), Some("\"team:scout\""));
            assert_eq!(
                value.decoded_not_for_display().as_deref(),
                Some("team:scout")
            );
        }
        other => panic!("{other:?}"),
    }
    let tools = blocks[1]
        .entries()
        .iter()
        .find_map(|entry| match entry {
            CardEntry::List { label, items } if label == labels::TOOLS => Some(items),
            _ => None,
        })
        .expect("the tools");
    let shown: Vec<&str> = tools.iter().filter_map(CardValue::quoted).collect();
    assert_eq!(shown, ["\"notes\\_read\"", "\"search\""]);
    let decoded: Vec<String> = tools
        .iter()
        .filter_map(CardValue::decoded_not_for_display)
        .collect();
    assert_eq!(decoded, ["notes_read", "search"]);

    // Consequences as a list, and the host's lines in their own block.
    assert_eq!(
        blocks[3].heading().map(CardEntry::label),
        Some(labels::CONSEQUENCES)
    );
    match &blocks[4].entries()[0] {
        CardEntry::List { label, items } => {
            assert_eq!(label, labels::HOST_CONSEQUENCES);
            let host: Vec<String> = items.iter().map(ToString::to_string).collect();
            assert_eq!(
                host,
                [
                    "\"Can message \\<the core team\\>\"",
                    "\"Reads private notes\""
                ]
            );
        }
        other => panic!("{other:?}"),
    }

    // Free text quoted line by line, escaped as written; decoded exactly,
    // for comparison only.
    let scout_text = blocks[6]
        .entries()
        .iter()
        .find(|entry| entry.label() == labels::INSTRUCTIONS)
        .expect("the scout's instructions");
    match scout_text {
        CardEntry::Text { lines, .. } => {
            assert_eq!(lines, &["Look around.", "", "Report \\[findings\\]."]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        scout_text.decoded_text_not_for_display().as_ref(),
        Some(&scout.instructions)
    );

    // The whole card covers every canonical part, for any part size.
    for size in [0, 1, 7, 64, 4096] {
        let max = part_size(size);
        assert_eq!(card.covered_parts(max), rendering.canonical_parts(max));
        assert!(shows_every_part(
            rendering.as_str(),
            max,
            &card.covered_parts(max)
        ));
    }
    // A card that leaves any block out covers fewer parts.
    let max = part_size(1);
    let every = rendering.canonical_parts(max);
    for left_out in 0..blocks.len() {
        let shown: Vec<usize> = (0..blocks.len())
            .filter(|index| *index != left_out)
            .collect();
        let covered = card.covered_parts_of(&shown, max);
        assert!(covered.len() < every.len(), "block {left_out}");
        assert!(!shows_every_part(rendering.as_str(), max, &covered));
    }
}

/// A displayed string shows no hidden character or line break, and every
/// markup character in it is escaped.
fn assert_displayed_escaped(shown: &str) {
    assert!(!shown.contains('\n'), "a line break in {shown:?}");
    assert!(
        !shown.chars().any(is_hidden),
        "a hidden character in {shown:?}"
    );
    let mut escaped = false;
    for character in shown.chars() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            assert!(!is_markup(character), "raw markup in {shown:?}");
        }
    }
}

#[test]
fn the_card_never_shows_hidden_characters_line_breaks_or_markup_raw() {
    let mut request = golden_create();
    {
        let spec = created_spec_mut(&mut request, "team:helper");
        spec.display_name = "x\u{202E}y".to_owned();
        spec.instructions = "a\u{E0041}b\u{200B}c".to_owned();
        spec.descriptive_labels = BTreeMap::from([
            (
                "note".to_owned(),
                "blue\u{2028}\u{2028}Host consequences (0):".to_owned(),
            ),
            ("multi".to_owned(), "a\nb".to_owned()),
        ]);
        spec.tools.insert("shell\u{202E}".to_owned());
    }
    request.consequences.insert(Consequence {
        identity: "team:helper".to_owned(),
        effect: ConsequenceEffect::ToolOutsideTemplate {
            tool: "shell\u{202E}".to_owned(),
        },
    });
    let hook = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["<b>x</b> ||y||"]))
    };
    let rendering = render_v1(&request, Some(&hook)).expect("a rendering");
    let card = rendering.card().expect("a card");

    let mut displayed = Vec::new();
    for entry in card.blocks().iter().flat_map(CardBlock::entries) {
        match entry {
            CardEntry::Field { value, .. } => displayed.push((value.to_string(), value.clone())),
            CardEntry::List { items, .. } => {
                displayed.extend(items.iter().map(|item| (item.to_string(), item.clone())));
            }
            CardEntry::Text { lines, .. } => {
                for line in lines {
                    assert_displayed_escaped(line);
                }
            }
        }
    }
    assert!(displayed.len() > 10);
    for (shown, value) in &displayed {
        assert_displayed_escaped(shown);
        for segment in value.segments() {
            if let ValueSegment::Quoted(text) = segment {
                assert!(
                    text.len() >= 2 && text.starts_with('"') && text.ends_with('"'),
                    "a quoted value keeps its quotes: {text}"
                );
            }
        }
    }

    // Each planted value shows as its visible escape.
    let shown: Vec<&str> = displayed.iter().map(|(shown, _)| shown.as_str()).collect();
    for expected in [
        "\"shell\\u{202E}\"",
        "\"note\" = \"blue\\u{2028}\\u{2028}Host consequences (0):\"",
        "\"multi\" = \"a\\u{000A}b\"",
        "\"\\<b\\>x\\</b\\> \\|\\|y\\|\\|\"",
    ] {
        assert!(shown.contains(&expected), "{expected} in {shown:?}");
    }
    let text_lines: Vec<String> = card
        .blocks()
        .iter()
        .flat_map(CardBlock::display_lines)
        .collect();
    assert!(text_lines.contains(&"> x\\u{202E}y".to_owned()));
    assert!(text_lines.contains(&"> a\\u{E0041}b\\u{200B}c".to_owned()));

    // The fake section is text inside a value, not a section of the card.
    let host_sections = card
        .blocks()
        .iter()
        .flat_map(CardBlock::entries)
        .filter(|entry| entry.label() == labels::HOST_CONSEQUENCES)
        .count();
    assert_eq!(host_sections, 1);

    // What the card displays is the canonical text, so its coverage is the
    // coverage of what was displayed.
    assert_eq!(card.covered_parts(None), rendering.canonical_parts(None));
}

#[test]
fn the_card_refuses_text_that_is_not_canonical() {
    let binding = binding_digest();
    let golden = GOLDEN_CREATE.replace("@BINDING@", binding.as_str());
    assert!(StructuredCard::parse(&golden).is_ok());
    let tampered = [
        golden.replace("\"notes\\_read\"", "\"notes_read\""),
        golden.replace("Tools (2):", "Tools (3):"),
        golden.replace("Tools (2):", "Tools (02):"),
        golden.replace("Display name (1 line):", "Display name (1 lines):"),
        golden.replace("render-v1", "render-v2"),
        golden.replace("> Helper", "> Hel\u{202E}per"),
        golden.replace("> Helper", "> \\u{0041}"),
        golden.replace("> Helper", "> "),
        golden.replace("Identities: 1\n", "Identities: 1\n\n"),
        golden.replace("Kickoff: none", "Kickoff:  none"),
        golden.replace("Kickoff: none", "Kickoff: \"none"),
        golden.trim_end().to_owned(),
        String::new(),
    ];
    for (index, text) in tampered.iter().enumerate() {
        assert_ne!(text, &golden, "tamper {index} changes the text");
        assert!(StructuredCard::parse(text).is_err(), "tamper {index}");
    }
}

// ---------------------------------------------------------------------------
// The host consequence hook
// ---------------------------------------------------------------------------

#[test]
fn a_consequence_hook_line_changes_the_subject_digest() {
    let request = golden_create();
    let without = render_v1(&request, None).expect("a rendering");
    let warning = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["Reads private notes"]))
    };
    let with = render_v1(&request, Some(&warning)).expect("a rendering");
    assert_ne!(with.subject_digest(), without.subject_digest());
    assert!(
        with.as_str()
            .contains("Host consequences (1):\n- \"Reads private notes\"\n")
    );
    let other = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["Reads every note"]))
    };
    assert_ne!(
        render_v1(&request, Some(&other))
            .expect("a rendering")
            .subject_digest(),
        with.subject_digest()
    );
    let none =
        |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> { Ok(Vec::new()) };
    assert_eq!(
        render_v1(&request, Some(&none)).expect("a rendering"),
        without,
        "a hook with no lines renders the platform's lines only"
    );
    let shuffled = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["b line", "a line", "b line"]))
    };
    let sorted = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["a line", "b line"]))
    };
    assert_eq!(
        render_v1(&request, Some(&shuffled)).expect("a rendering"),
        render_v1(&request, Some(&sorted)).expect("a rendering"),
        "host lines are sorted and deduplicated like every list"
    );
    let marked = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["||hidden|| [x](y)\nsecond line"]))
    };
    let escaped = render_v1(&request, Some(&marked)).expect("a rendering");
    assert!(
        escaped
            .as_str()
            .contains("- \"\\|\\|hidden\\|\\| \\[x\\](y)\\u{000A}second line\"\n")
    );
    assert_eq!(
        request.request_digest().expect("a digest"),
        golden_create().request_digest().expect("a digest"),
        "the hook's lines are the rendering's, not the request's"
    );
}

#[test]
fn a_failing_consequence_hook_refuses_and_renders_nothing() {
    let failing = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Err(HookRefusal::new("the warning catalog is offline"))
    };
    let refusal = render_v1(&golden_create(), Some(&failing)).expect_err("fails closed");
    assert_eq!(refusal.kind, IdentityRefusalKind::ConsequencesUnavailable);
    assert!(refusal.is_retryable());
    assert_eq!(
        refusal.to_data(),
        json!({"kind": "consequences_unavailable"})
    );
}

#[test]
fn oversize_host_consequence_lines_are_refused_typed() {
    let long_line = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(vec![ConsequenceLine::new(
            "x".repeat(HOST_CONSEQUENCE_MAX_BYTES + 1),
        )])
    };
    let refusal = render_v1(&golden_create(), Some(&long_line)).expect_err("a long line");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TextTooLarge {
            limit: HOST_CONSEQUENCE_MAX_BYTES as u64
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("host_consequences[0]"));
    let many_lines = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(
            (0..=HOST_CONSEQUENCES_MAX_BYTES / HOST_CONSEQUENCE_MAX_BYTES)
                .map(|index| {
                    ConsequenceLine::new(format!(
                        "{index:04}{}",
                        "x".repeat(HOST_CONSEQUENCE_MAX_BYTES - 4)
                    ))
                })
                .collect(),
        )
    };
    let refusal = render_v1(&golden_create(), Some(&many_lines)).expect_err("too many bytes");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TextTooLarge {
            limit: HOST_CONSEQUENCES_MAX_BYTES as u64
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("host_consequences"));
}

#[test]
fn the_consequences_callback_contract_round_trips() {
    let request = golden_create();
    let params = consequences_callback_params(&request).expect("params");
    assert_eq!(params["kind"], "identity.create");
    assert_eq!(params["request"], request.to_json().expect("json"));
    let read_back: IdentityRequest =
        serde_json::from_value(params["request"].clone()).expect("the request reads back");
    assert_eq!(read_back, request);
    assert_eq!(
        parse_consequences_callback_result(&json!({"lines": ["a", "b"]})).expect("lines"),
        lines(&["a", "b"])
    );
    for malformed in [
        json!({}),
        json!({"lines": [1]}),
        json!({"lines": [], "more": true}),
        json!(["a"]),
    ] {
        assert!(
            parse_consequences_callback_result(&malformed).is_err(),
            "{malformed}"
        );
    }
}

// ---------------------------------------------------------------------------
// The refusal vocabulary
// ---------------------------------------------------------------------------

#[test]
fn refusal_data_carries_the_kind_its_field_and_its_payload() {
    let cases = [
        (
            IdentityRefusal::new(IdentityRefusalKind::StaleAttempt { current: Some(4) }),
            json!({"kind": "stale_attempt", "current": 4}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::StaleAttempt { current: None }),
            json!({"kind": "stale_attempt"}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::ActivationPending { attempt: 2 }),
            json!({"kind": "activation_pending", "attempt": 2}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::ConstructionRequired),
            json!({"kind": "construction_required"}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::SubjectWidened).at("consequences"),
            json!({"kind": "subject_widened", "field": "consequences"}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::BatchTooLarge { max: 256 }),
            json!({"kind": "batch_too_large", "max": 256}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::Unsupported {
                feature: "an authority field schema".to_owned(),
            }),
            json!({"kind": "unsupported", "feature": "an authority field schema"}),
        ),
    ];
    for (refusal, data) in cases {
        assert_eq!(refusal.to_data(), data, "{refusal}");
    }
}
