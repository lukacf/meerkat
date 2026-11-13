//! Rows of the identity policy, `render_v1`, the canonical parts and the
//! structured card. Everything here is pure: no runtime, no store.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::sync::Arc;

use meerkat_contracts::WireHostRef;
use meerkat_core::{ContentBlock, ContentInput};
use meerkat_mob::{MobBackendKind, MobRuntimeMode, ProfileName};
use serde_json::{Value, json};

use super::escape::{escaped_line, is_hidden, is_markup, quoted, unescape_line, unquote};
use super::*;
use crate::identity_first::{AgentAddressability, AgentIdentity, DisplayName, DurableAgentSpec};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn policy_json() -> Value {
    json!({
        "enabled": true,
        "reserved_keys": {"labels": ["team", "clearance"]},
        "authority_fields": {"clearance": {"values": ["basic", "elevated"]}},
        "peers": {"rules": [
            {"id": "leads", "target": {"profile": ["lead"]}, "outcome": "allow"},
            {"id": "analysts", "target": {"profile": ["analyst"]}, "outcome": "allow"},
            {"id": "sensitive", "target": {"labels": {"team": "core"}},
             "outcome": "require_approval", "reason": "a core team member"}
        ], "default": "deny"},
        "approval": {"rules": [
            {"change": ["create"], "requester": {"kind": "agent"},
             "outcome": {"kind": "notify", "approver": {"id": "owner"}}},
            {"change": ["create"], "requester": {"kind": "host"}, "outcome": {"kind": "allow"}},
            {"change": ["capability", "peers", "authority"],
             "outcome": {"kind": "require_approval", "approver": {"id": "owner"}}},
            {"change": ["descriptive", "remove"],
             "outcome": {"kind": "notify", "approver": {"id": "owner"}}},
            {"change": ["profile"], "requester": {"kind": "host"}, "outcome": {"kind": "allow"}}
        ], "default": {"kind": "deny"}}
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

/// [`policy_json`] as a launch accepts it in this release: no rule names an
/// outcome that nothing carries out yet (each such rule asks for a
/// person's approval instead), and no profile is prepared.
fn served_policy_json() -> Value {
    let mut value = policy_json();
    if let Some(rules) = value["approval"]["rules"].as_array_mut() {
        for rule in rules {
            if matches!(rule["outcome"]["kind"].as_str(), Some("notify" | "review")) {
                rule["outcome"] = json!({
                    "kind": "require_approval",
                    "approver": rule["outcome"]["approver"].clone(),
                });
            }
        }
    }
    value
}

fn refusal_of(value: &Value) -> PolicyRefusal {
    IdentityCreationPolicy::parse(value).expect_err("a refused policy")
}

fn strings<const N: usize>(values: [&str; N]) -> BTreeSet<String> {
    values.into_iter().map(str::to_owned).collect()
}

/// The profiles of the mob definition.
fn mob_profiles() -> BTreeSet<String> {
    strings(["assistant", "analyst", "lead"])
}

fn lead() -> RequesterFacts {
    RequesterFacts::agent("lead-1", "lead", BTreeMap::new())
}

fn peers() -> BTreeMap<String, PeerFacts> {
    BTreeMap::from([
        (
            "lead-1".to_owned(),
            PeerFacts::new(Some("lead".to_owned()), BTreeMap::new()),
        ),
        (
            "analyst-1".to_owned(),
            PeerFacts::new(Some("analyst".to_owned()), BTreeMap::new()),
        ),
        (
            "core-1".to_owned(),
            PeerFacts::new(
                Some("assistant".to_owned()),
                BTreeMap::from([("team".to_owned(), "core".to_owned())]),
            ),
        ),
        (
            "stranger".to_owned(),
            PeerFacts::new(Some("assistant".to_owned()), BTreeMap::new()),
        ),
    ])
}

fn identity_of(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("an identity")
}

fn display_name_of(text: &str) -> DisplayName {
    DisplayName::parse(text).expect("a display name")
}

/// The declared spec of an identity of `profile`, as the request fixtures
/// ask for it.
fn declared(identity: &str, profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(identity).expect("an identity"),
        profile: ProfileName::from(profile),
        addressability: AgentAddressability::Addressable,
        display_name: Some(DisplayName::parse("Helper").expect("a display name")),
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec!["Read the notes.\nReport daily.".to_owned()],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

fn spec(identity: &str) -> CreateIdentitySpec {
    CreateIdentitySpec::new(declared(identity, "assistant"))
}

/// Ask for the profile `profile`.
fn set_profile(request: &mut CreateIdentitySpec, profile: &str) {
    request.spec.profile = ProfileName::from(profile);
}

/// Ask for the labels `entries`.
fn set_labels(request: &mut CreateIdentitySpec, entries: &[(&str, &str)]) {
    request.spec.labels = entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
}

/// Ask for the context `context`.
fn set_context(request: &mut CreateIdentitySpec, context: Value) {
    request.spec.context = Some(context);
}

/// Ask for the runtime binding `binding`.
fn set_binding(request: &mut CreateIdentitySpec, binding: Value) {
    request.spec.binding = Some(serde_json::from_value(binding).expect("a runtime binding"));
}

/// The one identity a create request creates.
fn only_created(request: &IdentityRequest) -> &CreatedSpec {
    match &request.action {
        RequestAction::Create { identities } => match identities.as_slice() {
            [created] => created,
            _ => panic!("one created identity"),
        },
        RequestAction::Update(_)
        | RequestAction::Remove { .. }
        | RequestAction::Archive { .. }
        | RequestAction::Unarchive { .. }
        | RequestAction::Adopt { .. } => panic!("a create request"),
    }
}

/// What the fixture's peer rules give the fixture peer `peer`.
fn peer_verdict(peer: &str) -> PeerVerdict {
    evaluate_peer(policy().peers(), peer, &peers()[peer])
}

/// An agent requester of the assistant profile with no authority label.
fn agent(identity: &str) -> RequesterFacts {
    RequesterFacts::agent(identity, "assistant", BTreeMap::new())
}

/// The labels `entries`.
fn labels_of(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// An agent requester of `profile` with the authority labels `labels`.
fn agent_of(identity: &str, profile: &str, labels: &[(&str, &str)]) -> RequesterFacts {
    RequesterFacts::agent(identity, profile, labels_of(labels))
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
    let profiles = mob_profiles();
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

fn golden_spec() -> CreatedSpec {
    CreatedSpec {
        spec: DurableAgentSpec {
            labels: BTreeMap::from([
                ("clearance".to_owned(), "elevated".to_owned()),
                ("color".to_owned(), "blue".to_owned()),
            ]),
            context: Some(json!({"topic": "stars", "depth": 2})),
            ..declared("helper", "assistant")
        },
        peers: BTreeMap::from([
            (
                "lead-1".to_owned(),
                EdgeBasis::Rule {
                    id: "leads".to_owned(),
                },
            ),
            (
                "core-1".to_owned(),
                EdgeBasis::Rule {
                    id: "sensitive".to_owned(),
                },
            ),
        ]),
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
            identities: vec![golden_spec()],
        },
        consequences: BTreeSet::from([
            Consequence {
                identity: "helper".to_owned(),
                effect: ConsequenceEffect::AuthorityField {
                    key: "clearance".to_owned(),
                    value: "elevated".to_owned(),
                },
            },
            Consequence {
                identity: "helper".to_owned(),
                effect: ConsequenceEffect::PeerNeedsApproval {
                    peer: "core-1".to_owned(),
                    rule: Some("sensitive".to_owned()),
                    reason: Some("a core team member".to_owned()),
                },
            },
        ]),
    }
}

/// A create by the host of an identity whose declared spec sets the fields
/// of how it runs and where, and an initial message of content blocks.
fn golden_runtime_create() -> IdentityRequest {
    let spec = DurableAgentSpec {
        addressability: AgentAddressability::InternalOnly,
        display_name: None,
        additional_instructions: Vec::new(),
        initial_message: Some(ContentInput::Blocks(vec![ContentBlock::Text {
            text: "Begin.".to_owned(),
        }])),
        runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
        backend: Some(MobBackendKind::External),
        binding: Some(
            serde_json::from_value(json!({
                "kind": "external",
                "address": "tcp://127.0.0.1:7000",
                "identity": {"kind": "ed25519_public_key", "public_key": "ed25519:AAAA"}
            }))
            .expect("a runtime binding"),
        ),
        placement: Some(WireHostRef("host-7".to_owned())),
        ..declared("scout", "analyst")
    };
    IdentityRequest {
        requester: Requester::Host {
            surface: "gateway".to_owned(),
        },
        proposer: None,
        source_evidence: None,
        action: RequestAction::Create {
            identities: vec![CreatedSpec {
                spec,
                peers: BTreeMap::new(),
            }],
        },
        consequences: BTreeSet::new(),
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
            identity: "helper".to_owned(),
            expected_spec_digest: helper_spec_digest(),
            profile: Some(ProfileChange {
                from: ProfileName::from("assistant"),
                to: ProfileName::from("analyst"),
            }),
            addressability: Some(AgentAddressability::InternalOnly),
            runtime_mode_override: None,
            display_name: None,
            additional_instructions: Some(vec!["Find better sources.".to_owned()]),
            context: None,
            descriptive_labels: None,
            authority_fields: None,
            peers_add: BTreeMap::from([(
                "analyst-1".to_owned(),
                EdgeBasis::Rule {
                    id: "analysts".to_owned(),
                },
            )]),
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
            identity: "helper".to_owned(),
            expected_spec_digest: helper_spec_digest(),
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
        RequestAction::Create { identities } => identities
            .iter()
            .find(|created| created.spec.identity.as_str() == identity)
            .expect("the identity"),
        RequestAction::Update(_)
        | RequestAction::Remove { .. }
        | RequestAction::Archive { .. }
        | RequestAction::Unarchive { .. }
        | RequestAction::Adopt { .. } => panic!("a create request"),
    }
}

fn created_spec_mut<'a>(request: &'a mut IdentityRequest, identity: &str) -> &'a mut CreatedSpec {
    match &mut request.action {
        RequestAction::Create { identities } => identities
            .iter_mut()
            .find(|created| created.spec.identity.as_str() == identity)
            .expect("the identity"),
        RequestAction::Update(_)
        | RequestAction::Remove { .. }
        | RequestAction::Archive { .. }
        | RequestAction::Unarchive { .. }
        | RequestAction::Adopt { .. } => panic!("a create request"),
    }
}

fn updated_mut(request: &mut IdentityRequest) -> &mut UpdatedSpec {
    match &mut request.action {
        RequestAction::Update(updated) => updated,
        RequestAction::Create { .. }
        | RequestAction::Remove { .. }
        | RequestAction::Archive { .. }
        | RequestAction::Unarchive { .. }
        | RequestAction::Adopt { .. } => panic!("an update request"),
    }
}

/// The fixture identity, of `profile` and with the authority labels
/// `authority`, as its owner reads it.
fn entry_with(profile: &str, authority: &[(&str, &str)]) -> IdentityEntryFacts {
    IdentityEntryFacts {
        identity: "helper".to_owned(),
        spec_digest: helper_spec_digest(),
        revision: Some(3),
        profile: profile.to_owned(),
        authority_labels: authority
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        labels: BTreeMap::new(),
    }
}

/// The fixture identity an update or a removal names.
fn entry() -> IdentityEntryFacts {
    entry_with("assistant", &[])
}

/// Any identity, whatever its profile or how it came to be.
fn any_entry() -> IdentityEntryFacts {
    entry_with("lead", &[])
}

fn update(
    requester: &RequesterFacts,
    entry: &IdentityEntryFacts,
    spec: &UpdateIdentitySpec,
) -> Result<NormalizedRequest, IdentityRefusal> {
    update_with(&policy(), requester, entry, spec)
}

/// An update under `policy`.
fn update_with(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    entry: &IdentityEntryFacts,
    spec: &UpdateIdentitySpec,
) -> Result<NormalizedRequest, IdentityRefusal> {
    let profiles = mob_profiles();
    let peers = peers();
    let context = RequestContext {
        requester,
        profiles: &profiles,
        peers: &peers,
    };
    normalize_update(policy, &context, RequestEnvelope::default(), entry, spec)
}

/// An update of `entry` at its current spec, changing `set`.
fn update_of(entry: &IdentityEntryFacts, set: Value) -> UpdateIdentitySpec {
    serde_json::from_value(json!({
        "identity": entry.identity,
        "expected_spec_digest": entry.spec_digest,
        "set": set,
    }))
    .expect("an update request")
}

/// A removal of `entry` at its current spec by `requester`, under `policy`.
fn remove_identity_with(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    entry: &IdentityEntryFacts,
) -> Result<NormalizedRequest, IdentityRefusal> {
    let spec = RemoveIdentitySpec {
        identity: entry.identity.clone(),
        expected_spec_digest: entry.spec_digest.clone(),
    };
    normalize_remove(policy, requester, RequestEnvelope::default(), entry, &spec)
}

/// A removal of `entry` at its current spec by `requester`.
fn remove_identity(
    requester: &RequesterFacts,
    entry: &IdentityEntryFacts,
) -> Result<NormalizedRequest, IdentityRefusal> {
    remove_identity_with(&policy(), requester, entry)
}

/// The policy a launch of the fixture mob uses, from the native host's
/// answer (`None`: no native host) and the init option.
fn resolve(
    host_policy: Option<Result<Option<IdentityCreationPolicy>, PolicyRefusal>>,
    option: RuntimeOptionPolicy,
    persistent: bool,
) -> Result<Option<Arc<IdentityCreationPolicy>>, PolicyRefusal> {
    resolve_launch_policy(host_policy, option, &mob_profiles(), persistent)
}

/// The preparation the policy gives a new identity of `profile`.
fn preparation_of(policy: &IdentityCreationPolicy, profile: &str) -> PreparationMode {
    policy.preparation(profile)
}

/// The setup deadline of a held construction of `profile`, in milliseconds.
fn deadline_of(policy: &IdentityCreationPolicy, profile: &str) -> Option<u64> {
    policy.construction_deadline_ms(profile)
}

/// The digest of the fixture identity's declared spec.
fn helper_spec_digest() -> Sha256Digest {
    Sha256Digest::of(b"the spec of helper")
}

/// The spec digest `entry` has now, as its owner reads it.
fn spec_digest_of(entry: &IdentityEntryFacts) -> String {
    entry.spec_digest.to_string()
}

/// `spec`, asked of a spec the identity no longer has.
fn stale_update(spec: &UpdateIdentitySpec) -> UpdateIdentitySpec {
    UpdateIdentitySpec {
        expected_spec_digest: Sha256Digest::of(b"an older spec of helper"),
        ..spec.clone()
    }
}

/// A removal of `entry` by `requester`, asked of a spec it no longer has.
fn stale_removal(
    requester: &RequesterFacts,
    entry: &IdentityEntryFacts,
) -> Result<NormalizedRequest, IdentityRefusal> {
    let spec = RemoveIdentitySpec {
        identity: entry.identity.clone(),
        expected_spec_digest: Sha256Digest::of(b"an older spec of helper"),
    };
    normalize_remove(
        &policy(),
        requester,
        RequestEnvelope::default(),
        entry,
        &spec,
    )
}

// ---------------------------------------------------------------------------
// Policy validation rows
// ---------------------------------------------------------------------------

type PolicyChange = fn(&mut Value);

fn policy_case(
    change: PolicyChange,
    path: &'static str,
    kind: PolicyRefusalKind,
) -> (PolicyChange, &'static str, PolicyRefusalKind) {
    (change, path, kind)
}

fn path_case(change: PolicyChange, path: &'static str) -> (PolicyChange, &'static str) {
    (change, path)
}

#[test]
fn policy_validation_refuses_typed() {
    // A bare enabled policy validates with the defaults: deny for the
    // changes and the peers no rule governs, the decision timeouts, no
    // preparation and no authority field.
    let bare = parse(&json!({"enabled": true}));
    assert_eq!(bare.approval().default_outcome().kind(), ApprovalKind::Deny);
    assert_eq!(bare.peers().default_outcome(), PeerOutcome::Deny);
    let timeouts = bare.approval().timeouts();
    assert_eq!(timeouts.create, 86_400_000);
    assert_eq!(timeouts.capability, 14_400_000);
    assert_eq!(timeouts.profile, 14_400_000);
    assert_eq!(timeouts.max, MAX_DECISION_TIMEOUT_MS);
    assert_eq!(preparation_of(&bare, "assistant"), PreparationMode::None);
    assert!(bare.authority_fields().is_empty());
    let policy = policy();
    assert!(policy.is_reserved_label("team"));
    assert!(
        policy.is_reserved_label("role"),
        "Meta's own keys are reserved"
    );
    assert!(
        policy
            .authority_field("clearance")
            .is_some_and(|field| field.allows("elevated"))
    );
    assert!(
        policy.authority_field("team").is_none(),
        "a reserved key is no authority field unless the policy declares one"
    );
    // The revision is stable, reads the defaults as written out, and
    // changes with the policy.
    assert_eq!(policy.revision(), parse(&policy_json()).revision());
    let mut explicit = policy_json();
    explicit["preparation"] = json!({});
    assert_eq!(
        parse(&explicit).revision(),
        policy.revision(),
        "defaults are explicit"
    );
    let mut changed = policy_json();
    changed["approval"]["default"] = json!({"kind": "allow"});
    assert_ne!(parse(&changed).revision(), policy.revision());
    // A disabled policy is validated all the same, and yields no policy.
    assert_eq!(IdentityCreationPolicy::parse(&json!({})).unwrap(), None);
    let mut disabled = policy_json();
    disabled["enabled"] = json!(false);
    assert_eq!(IdentityCreationPolicy::parse(&disabled).unwrap(), None);
    disabled["preparation"] = json!({"analyst": {"mode": "constructed"}});
    assert_eq!(
        refusal_of(&disabled).path,
        "preparation.analyst.construction_deadline_ms"
    );

    let cases = [
        policy_case(
            |value| value["authority_fields"] = json!({"mood": {"values": ["calm"]}}),
            "authority_fields.mood",
            PolicyRefusalKind::AuthorityFieldNotReserved {
                key: "mood".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["authority_fields"] = json!({"clearance": {"schema": {"type": "string"}}});
            },
            "authority_fields.clearance.schema",
            PolicyRefusalKind::Unsupported {
                feature: "an authority field schema".to_owned(),
            },
        ),
        policy_case(
            |value| value["authority_fields"] = json!({"clearance": {"values": []}}),
            "authority_fields.clearance.values",
            PolicyRefusalKind::Empty,
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
            |value| {
                value["approval"]["rules"][0]["requester"] =
                    json!({"kind": "agent", "identity": ["lead-1", "lead-1"]});
            },
            "approval.rules[0].requester.identity[1]",
            PolicyRefusalKind::Duplicate {
                value: "lead-1".to_owned(),
            },
        ),
        policy_case(
            |value| {
                value["approval"]["rules"][1]["requester"] =
                    json!({"kind": "host", "surface": "the console"});
            },
            "approval.rules[1].requester.surface",
            PolicyRefusalKind::Invalid {
                value: "the console".to_owned(),
            },
        ),
        policy_case(
            |value| value["approval"]["rules"][0]["target"] = json!({"labels": {"bad key": "x"}}),
            "approval.rules[0].target.labels",
            PolicyRefusalKind::Invalid {
                value: "bad key".to_owned(),
            },
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
            |value| value["reserved_keys"]["labels"] = json!(["team", "team"]),
            "reserved_keys.labels[1]",
            PolicyRefusalKind::Duplicate {
                value: "team".to_owned(),
            },
        ),
        policy_case(
            |value| value["preparation"] = json!({"analyst": {"mode": "constructed"}}),
            "preparation.analyst.construction_deadline_ms",
            PolicyRefusalKind::Empty,
        ),
        policy_case(
            |value| {
                value["preparation"] =
                    json!({"analyst": {"mode": "required", "construction_deadline_ms": 5000}});
            },
            "preparation.analyst.construction_deadline_ms",
            PolicyRefusalKind::Invalid {
                value: "5000".to_owned(),
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

    // Meta's own label keys are Meta's: a policy neither reserves one nor
    // declares it an authority field, so no request ever sets one.
    let meta_keys = [
        path_case(
            |value| value["reserved_keys"]["labels"] = json!(["team", "role"]),
            "reserved_keys.labels[1]",
        ),
        path_case(
            |value| value["authority_fields"] = json!({"agent_identity": {"values": ["x"]}}),
            "authority_fields.agent_identity",
        ),
    ];
    for (change, path) in meta_keys {
        let mut value = policy_json();
        change(&mut value);
        let refusal = refusal_of(&value);
        assert_eq!(refusal.path, path, "{refusal}");
        assert!(refusal.kind.to_string().contains("Meta's own"), "{refusal}");
    }

    // A member that does not parse is refused malformed at its path, and so
    // is each section of a policy for created identities only.
    let malformed = [
        (json!({"enabled": true, "surprise": 1}), "", "surprise"),
        (json!({"enabled": true, "templates": {}}), "", "templates"),
        (json!({"enabled": true, "creators": []}), "", "creators"),
        (json!({"enabled": true, "limits": {}}), "", "limits"),
        (json!({"enabled": true, "naming": {}}), "", "naming"),
        (
            json!({"enabled": true, "console": {"templates": []}}),
            "",
            "console",
        ),
        (
            json!({"enabled": true, "authority_fields": {"clearance": {"valuse": []}}}),
            "authority_fields.clearance",
            "valuse",
        ),
        (
            json!({"enabled": true, "peers": {"rules": [{"id": "a", "relation": "creator"}]}}),
            "peers.rules[0]",
            "relation",
        ),
        (
            json!({"enabled": true, "approval": {"rules": [{"change": ["create"]}, {"chnage": []}]}}),
            "approval.rules[1]",
            "chnage",
        ),
        (
            json!({"enabled": true, "approval": {"rules": [{"change": ["rebind"]}]}}),
            "approval.rules[0]",
            "rebind",
        ),
        (
            json!({"enabled": true, "approval": {"rules": [{"change": ["create"], "creator": {}}]}}),
            "approval.rules[0]",
            "creator",
        ),
        (json!({"enabled": "yes"}), "enabled", "boolean"),
        (
            json!({"enabled": true, "reserved_keys": {"labels": [], "context": ["project"]}}),
            "reserved_keys",
            "context",
        ),
        (
            json!({"enabled": true, "construction_deadline_ms": 60_000}),
            "",
            "construction_deadline_ms",
        ),
        (
            json!({"enabled": true, "preparation": "constructed"}),
            "preparation",
            "map",
        ),
        (
            json!({"enabled": true, "preparation": {"analyst": {"mode": "required", "deadline": 1}}}),
            "preparation.analyst",
            "deadline",
        ),
        (
            json!({"enabled": true, "approval": {"rules": [{"change": ["create"], "requester": ["agent"]}]}}),
            "approval.rules[0]",
            "sequence",
        ),
    ];
    for (value, path, detail) in malformed {
        let refusal = refusal_of(&value);
        let PolicyRefusalKind::Malformed { detail: shown } = &refusal.kind else {
            panic!("{value}: {refusal}");
        };
        assert_eq!(refusal.path, path, "{value}: {refusal}");
        assert!(shown.contains(detail), "{value}: {refusal}");
        assert!(refusal.to_string().starts_with("identity_creation"));
    }
}

#[test]
fn a_batch_above_cores_transaction_maximum_is_refused() {
    let host = RequesterFacts::host("gateway");
    let specs: Vec<CreateIdentitySpec> = (0..=CORE_BATCH_MAX)
        .map(|n| spec(&format!("helper-{n}")))
        .collect();
    let refusal = create(&policy(), &host, &specs).expect_err("over the transaction maximum");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::BatchTooLarge {
            max: CORE_BATCH_MAX
        }
    );
    assert_eq!(refusal.to_data()["kind"], "batch_too_large");
    assert_eq!(refusal.to_data()["max"], CORE_BATCH_MAX);
    // One transaction's worth is a create like any other: the policy sets
    // no limit of its own below it.
    let full: Vec<CreateIdentitySpec> = (0..CORE_BATCH_MAX)
        .map(|n| spec(&format!("helper-{n}")))
        .collect();
    assert!(create(&policy(), &host, &full).is_ok());
}

#[test]
fn a_constructed_preparation_needs_its_deadline_and_a_native_host() {
    // Preparation is the profile's: each profile the policy names has its
    // mode, and a profile it does not name needs none.
    let mut value = policy_json();
    value["preparation"] = json!({
        "analyst": {"mode": "constructed", "construction_deadline_ms": 60_000},
        "lead": {"mode": "required"}
    });
    let policy = parse(&value);
    assert!(policy.needs_native_host());
    assert_eq!(
        preparation_of(&policy, "analyst"),
        PreparationMode::Constructed
    );
    assert_eq!(deadline_of(&policy, "analyst"), Some(60_000));
    assert_eq!(preparation_of(&policy, "lead"), PreparationMode::Required);
    assert_eq!(deadline_of(&policy, "lead"), None);
    assert_eq!(
        preparation_of(&policy, "assistant"),
        PreparationMode::None,
        "a profile the policy does not name needs no preparation"
    );
    // No launch of this release holds or activates a construction: the
    // policy is refused at the constructed preparation's mode, with a text
    // that names the mode a host writes, whether the launch has a native
    // host or not. What is not served is said before what a host lacks.
    for native_host in [false, true] {
        let unserved = resolve(
            native_host.then_some(Ok(None)),
            RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
            true,
        )
        .expect_err("no construction is held in this release");
        assert_eq!(
            unserved.path, "preparation.analyst.mode",
            "native host: {native_host}"
        );
        assert!(
            matches!(&unserved.kind, PolicyRefusalKind::Unsupported { feature }
                if feature.contains("constructed") && feature.contains("write required")),
            "native host: {native_host}: {unserved}"
        );
    }
    // A required preparation needs no native host and is served: the host
    // settles it. The policy is one a launch serves in every other
    // respect too (no rule names an outcome nothing carries out).
    let mut served_value = served_policy_json();
    served_value["preparation"] = json!({"analyst": {"mode": "required"}});
    let required = parse(&served_value);
    assert!(!required.needs_native_host());
    let served = resolve(None, RuntimeOptionPolicy::Enabled(Box::new(required)), true)
        .expect("a launch serves a required preparation")
        .expect("the policy is the launch's");
    assert_eq!(
        preparation_of(&served, "analyst"),
        PreparationMode::Required
    );
    value["preparation"] =
        json!({"analyst": {"mode": "constructed", "construction_deadline_ms": 10}});
    let refusal = refusal_of(&value);
    assert_eq!(refusal.path, "preparation.analyst.construction_deadline_ms");
    assert!(matches!(refusal.kind, PolicyRefusalKind::OutOfRange { .. }));
}

#[test]
fn a_launch_without_persistence_refuses_a_policy_from_either_source() {
    let policy = policy();
    for (host_policy, option) in [
        (None, RuntimeOptionPolicy::Enabled(Box::new(policy.clone()))),
        (None, RuntimeOptionPolicy::Disabled),
        (Some(Ok(None)), RuntimeOptionPolicy::Disabled),
        (Some(Ok(Some(policy.clone()))), RuntimeOptionPolicy::Absent),
    ] {
        let refusal = resolve(host_policy, option, false).expect_err("no persistence");
        assert_eq!(refusal.kind, PolicyRefusalKind::NeedsPersistentLaunch);
        assert_eq!(refusal.path, "");
    }
    for host_policy in [None, Some(Ok(None))] {
        assert!(
            resolve(host_policy, RuntimeOptionPolicy::Absent, false)
                .expect("nothing given, nothing refused")
                .is_none()
        );
    }
    let both = resolve(
        Some(Ok(Some(policy.clone()))),
        RuntimeOptionPolicy::Enabled(Box::new(policy)),
        false,
    )
    .expect_err("two sources");
    assert_eq!(both.kind, PolicyRefusalKind::TwoSources);
}

#[test]
fn the_policy_comes_from_the_option_or_the_host_never_both() {
    let policy = parse(&served_policy_json());
    let from_option = resolve(
        None,
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
        true,
    )
    .unwrap();
    assert_eq!(from_option.as_deref(), Some(&policy));
    let from_host = resolve(
        Some(Ok(Some(policy.clone()))),
        RuntimeOptionPolicy::Absent,
        true,
    )
    .unwrap();
    assert_eq!(
        from_host.as_deref(),
        Some(&policy),
        "the native host's policy is used when the option is absent"
    );
    let host_without_one = resolve(
        Some(Ok(None)),
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
        true,
    )
    .unwrap();
    assert_eq!(host_without_one.as_deref(), Some(&policy));
    for option in [
        RuntimeOptionPolicy::Disabled,
        RuntimeOptionPolicy::Enabled(Box::new(policy.clone())),
    ] {
        let both = resolve(Some(Ok(Some(policy.clone()))), option, true)
            .expect_err("two sources, a disabled option included");
        assert_eq!(both.kind, PolicyRefusalKind::TwoSources);
        assert_eq!(both.path, "");
    }
    let host_refusal = PolicyRefusal::new("approval.rules[0].change", PolicyRefusalKind::Empty);
    assert_eq!(
        resolve(
            Some(Err(host_refusal.clone())),
            RuntimeOptionPolicy::Absent,
            true
        )
        .unwrap_err(),
        host_refusal,
        "the native host's refusal refuses the launch unchanged"
    );
    for option in [RuntimeOptionPolicy::Absent, RuntimeOptionPolicy::Disabled] {
        assert!(
            resolve(None, option, true)
                .expect("no policy, no refusal")
                .is_none()
        );
    }
}

#[test]
fn the_launch_refuses_a_profile_the_mob_does_not_define() {
    // A preparation key or a selector profile that names no profile of the
    // mob is refused at launch: a misspelled one would seat that profile's
    // identities unprepared, or leave a host's deny rule matching nothing.
    let cases = [
        path_case(
            |value| value["approval"]["rules"][1]["target"] = json!({"profile": ["ghost"]}),
            "approval.rules[1].target.profile",
        ),
        path_case(
            |value| value["peers"]["rules"][0]["target"] = json!({"profile": ["ghost"]}),
            "peers.rules[0].target.profile",
        ),
        path_case(
            |value| {
                value["approval"]["rules"][0]["requester"] =
                    json!({"kind": "agent", "profile": ["ghost"]});
            },
            "approval.rules[0].requester.profile",
        ),
        path_case(
            |value| value["preparation"] = json!({"ghost": {"mode": "required"}}),
            "preparation.ghost",
        ),
    ];
    for (change, path) in cases {
        let mut value = policy_json();
        change(&mut value);
        let option = RuntimeOptionPolicy::parse(&value).expect("a valid policy");
        let refusal = resolve(None, option, true).expect_err("a profile the mob lacks");
        assert_eq!(refusal.path, path, "{refusal}");
        assert!(
            refusal
                .to_string()
                .ends_with("profile \"ghost\" is not in the mob definition"),
            "{refusal}"
        );
    }
    // A native host's policy is checked the same way.
    let mut value = policy_json();
    value["approval"]["rules"][1]["target"] = json!({"profile": ["ghost"]});
    let refusal = resolve(
        Some(Ok(Some(parse(&value)))),
        RuntimeOptionPolicy::Absent,
        true,
    )
    .expect_err("the host's policy is checked too");
    assert_eq!(refusal.path, "approval.rules[1].target.profile");
    // Every profile of the mob may be named.
    let mut known = served_policy_json();
    known["preparation"] = json!({"lead": {"mode": "none"}});
    known["approval"]["rules"][1]["target"] = json!({"profile": ["assistant", "analyst", "lead"]});
    let option = RuntimeOptionPolicy::parse(&known).expect("a valid policy");
    assert!(resolve(None, option, true).unwrap().is_some());
}

// ---------------------------------------------------------------------------
// Normalization rows
// ---------------------------------------------------------------------------

#[test]
fn a_request_normalizes_to_its_declared_spec_and_plan() {
    let mut request = spec("helper");
    set_profile(&mut request, "analyst");
    let normalized = create(&policy(), &lead(), &[request]).expect("normalized");
    assert_eq!(
        normalized.request.to_json().expect("the request body")["action"]["identities"],
        json!([{"spec": declared("helper", "analyst"), "peers": {}}]),
        "the request holds the declared spec as asked: its own name and the requested \
         profile, and no namespace, template binding, model or tool list"
    );
    assert_eq!(
        normalized.plan.outcome,
        PlannedOutcome::Notify {
            approver_id: "owner".to_owned()
        }
    );
    assert_eq!(normalized.plan.change, ChangeClass::Create);
    assert_eq!(normalized.plan.timeout_ms, 86_400_000);
    assert!(!normalized.plan.raised);
    assert!(normalized.request.consequences.is_empty());
    assert_eq!(normalized.request.decision_kind(), "identity.create");
    let host = create(
        &policy(),
        &RequesterFacts::host("gateway"),
        &[spec("helper")],
    )
    .expect("normalized");
    assert_eq!(host.plan.outcome, PlannedOutcome::Allow);
}

type SpecChange = fn(&mut CreateIdentitySpec);

fn spec_case(change: SpecChange, refusal: Value) -> (SpecChange, Value) {
    (change, refusal)
}

#[test]
fn normalization_refuses_typed() {
    // A request may ask for any profile of the mob: no template narrows it.
    let mut lead_profile = spec("helper");
    set_profile(&mut lead_profile, "lead");
    assert!(
        create(&policy(), &lead(), &[lead_profile]).is_ok(),
        "any mob profile is allowed"
    );
    // Context is no authority input: it may name any key, a reserved label
    // key included, and no label key is reserved for context.
    let mut free_context = spec("helper");
    set_context(
        &mut free_context,
        json!({"team": "core", "project": "stars"}),
    );
    set_labels(&mut free_context, &[("project", "stars")]);
    assert!(
        create(&policy(), &lead(), &[free_context]).is_ok(),
        "context keys are free"
    );
    // A label value has no length limit of its own: the request body's bound
    // is the only one.
    let mut long_value = spec("helper");
    set_labels(&mut long_value, &[("note", "x".repeat(4096).as_str())]);
    assert!(
        create(&policy(), &lead(), &[long_value]).is_ok(),
        "a long label value is a label like any other"
    );

    let cases = [
        spec_case(
            |spec| set_profile(spec, "ghost"),
            json!({"kind": "profile_unknown", "profile": "ghost",
                   "field": "agents[0].profile"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("team", "x")]),
            json!({"kind": "reserved_key", "key": "team", "field": "agents[0].labels"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("role", "x")]),
            json!({"kind": "reserved_key", "key": "role", "field": "agents[0].labels"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("bad key", "x")]),
            json!({"kind": "invalid_request", "problem": "malformed",
                   "field": "agents[0].labels"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("", "x")]),
            json!({"kind": "invalid_request", "problem": "malformed",
                   "field": "agents[0].labels"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("note\u{202E}", "x")]),
            json!({"kind": "invalid_request", "problem": "malformed",
                   "field": "agents[0].labels"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("note", "a\nb")]),
            json!({"kind": "invalid_request", "problem": "malformed",
                   "field": "agents[0].labels.note"}),
        ),
        spec_case(
            |spec| set_labels(spec, &[("clearance", "top")]),
            json!({"kind": "authority_field_invalid", "key": "clearance",
                   "field": "agents[0].labels"}),
        ),
        spec_case(
            |spec| {
                set_binding(
                    spec,
                    json!({"kind": "external", "address": "tcp://127.0.0.1:7000",
                           "bootstrap_token": "secret-token",
                           "identity": {"kind": "ed25519_public_key",
                                        "public_key": "ed25519:AAAA"}}),
                );
            },
            json!({"kind": "unsupported", "feature": "a bootstrap token in an identity request",
                   "field": "agents[0]"}),
        ),
        spec_case(
            |spec| spec.peers = vec!["ghost".to_owned()],
            json!({"kind": "peer_unknown", "field": "agents[0].peers[0]"}),
        ),
        spec_case(
            |spec| spec.peers = vec!["stranger".to_owned()],
            json!({"kind": "peer_not_allowed", "identity": "stranger",
                   "field": "agents[0].peers[0]"}),
        ),
        spec_case(
            |spec| spec.peers = vec!["helper".to_owned()],
            json!({"kind": "peer_not_allowed", "identity": "helper",
                   "field": "agents[0].peers[0]"}),
        ),
    ];
    for (change, expected) in cases {
        let mut request = spec("helper");
        change(&mut request);
        let refusal = create(&policy(), &lead(), &[request]).expect_err("refused");
        assert_eq!(refusal.to_data(), expected, "{refusal}");
        assert!(!refusal.is_retryable());
    }

    let refusal =
        create(&policy(), &lead(), &[spec("helper"), spec("helper")]).expect_err("a duplicate");
    assert_eq!(
        refusal.to_data(),
        json!({"kind": "name_taken", "by": "identity", "field": "agents[1].name"})
    );

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
        refusal.to_data(),
        json!({"kind": "change_denied", "change": "create"}),
        "no governing rule gets the default, deny"
    );

    // Meta's own label keys are set by no request: not as a label, and not
    // as an authority field, before any authority field is looked up.
    let entry = entry();
    let meta_key = update_of(&entry, json!({"authority_fields": {"role": "admin"}}));
    assert_eq!(
        update(&RequesterFacts::host("gateway"), &entry, &meta_key)
            .unwrap_err()
            .to_data(),
        json!({"kind": "reserved_key", "key": "role", "field": "set.labels"})
    );
}

#[test]
fn a_create_spec_with_a_member_the_declared_spec_lacks_is_refused() {
    // The declared spec is closed on the request side: a member it does not
    // have (a tool list, a template, a model, a misspelled field) is refused
    // before the request is normalized, never dropped.
    let value = serde_json::to_value(spec("helper")).expect("a request");
    let read: CreateIdentitySpec =
        serde_json::from_value(value.clone()).expect("every member of the declared spec");
    assert_eq!(read, spec("helper"));
    for member in ["tools", "template", "model", "labls"] {
        let mut changed = value.clone();
        changed["spec"][member] = json!("x");
        let error = serde_json::from_value::<CreateIdentitySpec>(changed)
            .expect_err("a member the declared spec lacks");
        assert!(error.to_string().contains(member), "{error}");
    }
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
    let rule = |id: &str| EdgeBasis::Rule { id: id.to_owned() };
    assert_eq!(
        only_created(&normalized.request).peers,
        BTreeMap::from([
            ("analyst-1".to_owned(), rule("analysts")),
            ("core-1".to_owned(), rule("sensitive")),
            ("lead-1".to_owned(), rule("leads")),
        ]),
        "every edge has the basis of the rule that allowed it: the requester is no basis, \
         and neither is a peer of the requester"
    );
    // The peer that needs approval raises the request to a human decision,
    // and the deciding rule's approver decides it.
    assert!(normalized.plan.raised);
    assert_eq!(
        normalized.plan.outcome,
        PlannedOutcome::RequireApproval {
            approver_id: "owner".to_owned()
        }
    );
    let rendering = render(&normalized.request);
    assert!(rendering.as_str().contains(
        "- \"helper\" peer \"core-1\" needs approval under rule \"sensitive\" because \
         \"a core team member\"\n"
    ));

    // A raised request whose deciding rule names no approver has nobody to
    // decide, and is refused before anything is held.
    let mut core_peer = spec("helper");
    core_peer.peers = vec!["core-1".to_owned()];
    let refusal = create(&policy(), &RequesterFacts::host("gateway"), &[core_peer])
        .expect_err("nobody can decide");
    assert_eq!(refusal.kind, IdentityRefusalKind::ApprovalUnavailable);

    assert_eq!(
        peer_verdict("stranger"),
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
    set_labels(&mut request, &[("clearance", "elevated")]);
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
        identity: "helper".to_owned(),
        effect: ConsequenceEffect::AuthorityField {
            key: "clearance".to_owned(),
            value: "elevated".to_owned()
        },
    }));
    assert!(
        render(&normalized.request).as_str().contains(
            "- \"helper\" authority field \"clearance\" = \"elevated\" becomes authority\n"
        )
    );

    // The authority fields are the policy's, for any identity: an update
    // sets one under a human decision too, and only a declared one.
    let entry = entry();
    let host = RequesterFacts::host("gateway");
    let basic = update_of(&entry, json!({"authority_fields": {"clearance": "basic"}}));
    let normalized = update(&host, &entry, &basic).expect("a human decides");
    assert_eq!(normalized.plan.kind(), ApprovalKind::RequireApproval);
    assert!(normalized.request.consequences.contains(&Consequence {
        identity: "helper".to_owned(),
        effect: ConsequenceEffect::AuthorityField {
            key: "clearance".to_owned(),
            value: "basic".to_owned()
        },
    }));
    let undeclared = update_of(&entry, json!({"authority_fields": {"team": "core"}}));
    assert_eq!(
        update(&host, &entry, &undeclared).unwrap_err().kind,
        IdentityRefusalKind::AuthorityFieldInvalid {
            key: "team".to_owned()
        }
    );

    // Removing authority fields narrows: it gets its change class's
    // outcome, here the default allow, and needs no human decision.
    let mut narrowing = policy_json();
    narrowing["approval"]["rules"][2]["change"] = json!(["capability", "peers"]);
    narrowing["approval"]["default"] = json!({"kind": "allow"});
    let narrowing = parse(&narrowing);
    let elevated = entry_with("assistant", &[("clearance", "elevated")]);
    let removal = update_of(&elevated, json!({"authority_fields": {}}));
    let normalized = update_with(&narrowing, &lead(), &elevated, &removal).expect("a narrowing");
    assert_eq!(normalized.plan.kind(), ApprovalKind::Allow);
}

#[test]
fn a_label_key_a_selector_reads_is_reserved() {
    // The host reserves no key here, yet every key a selector reads is
    // reserved: an approval rule's target, a peer rule's target and a label
    // approver. A request sets one only as an authority field, under a human
    // decision, so selectors read authority labels only.
    let policy = parse(&json!({
        "enabled": true,
        "peers": {"rules": [
            {"id": "north", "target": {"labels": {"site": "north"}}, "outcome": "allow"}
        ]},
        "approval": {"rules": [
            {"change": ["create", "descriptive"], "outcome": {"kind": "allow"}},
            {"change": ["create", "descriptive"], "target": {"labels": {"tier": "prod"}},
             "outcome": {"kind": "deny"}},
            {"change": ["remove"],
             "outcome": {"kind": "notify", "approver": {"label": "manager"}}}
        ]}
    }));
    let host = RequesterFacts::host("gateway");
    let entry = entry();
    for key in ["tier", "site", "manager"] {
        assert!(policy.is_reserved_label(key), "{key}");
        let mut request = spec("helper");
        set_labels(&mut request, &[(key, "x")]);
        assert_eq!(
            create(&policy, &host, &[request]).unwrap_err().to_data(),
            json!({"kind": "reserved_key", "key": key, "field": "agents[0].labels"}),
            "{key}"
        );
        let mut set = json!({"descriptive_labels": {}});
        set["descriptive_labels"][key] = json!("x");
        let relabel = update_of(&entry, set);
        assert_eq!(
            update_with(&policy, &host, &entry, &relabel)
                .unwrap_err()
                .to_data(),
            json!({"kind": "reserved_key", "key": key, "field": "set.labels"}),
            "{key}"
        );
    }
    let mut free = spec("helper");
    set_labels(&mut free, &[("color", "blue")]);
    assert!(
        create(&policy, &host, &[free]).is_ok(),
        "a key no selector reads is free"
    );
    // The deny rule reads the key from the authority labels the identity's
    // owner holds: an identity that carries it is denied, another is not.
    let prod = entry_with("assistant", &[("tier", "prod")]);
    let rename = update_of(&prod, json!({"display_name": "Helper two"}));
    assert_eq!(
        update_with(&policy, &host, &prod, &rename)
            .unwrap_err()
            .to_data(),
        json!({"kind": "change_denied", "change": "descriptive"})
    );
    let rename = update_of(&entry, json!({"display_name": "Helper two"}));
    assert_eq!(
        update_with(&policy, &host, &entry, &rename)
            .expect("no selector reads this identity's labels")
            .plan
            .kind(),
        ApprovalKind::Allow
    );
}

#[test]
fn the_requester_selector_reads_kind_surface_identity_profile_and_labels() {
    // A rule selects who asks by kind, host surface, agent identity, profile
    // and authority labels: a host rule for the console surface governs the
    // console and no other surface of the host.
    let policy = parse(&json!({
        "enabled": true,
        "approval": {"rules": [
            {"change": ["descriptive"], "requester": {"kind": "host", "surface": "console"},
             "outcome": {"kind": "notify", "approver": {"id": "owner"}}},
            {"change": ["descriptive"], "requester": {"kind": "host", "surface": "gateway"},
             "outcome": {"kind": "allow"}},
            {"change": ["descriptive"], "requester": {"kind": "agent", "profile": ["lead"]},
             "outcome": {"kind": "allow"}},
            {"change": ["descriptive"], "requester": {"identity": ["analyst-1"]},
             "outcome": {"kind": "allow"}},
            {"change": ["descriptive"], "requester": {"labels": {"desk": "ops"}},
             "outcome": {"kind": "require_approval", "approver": {"id": "security"}}}
        ]}
    }));
    let entry = entry();
    let rename = update_of(&entry, json!({"display_name": "Helper two"}));
    let planned = |requester: &RequesterFacts| {
        update_with(&policy, requester, &entry, &rename).map(|normalized| normalized.plan.outcome)
    };
    let denied = json!({"kind": "change_denied", "change": "descriptive"});
    assert_eq!(
        planned(&RequesterFacts::console()),
        Ok(PlannedOutcome::Notify {
            approver_id: "owner".to_owned()
        })
    );
    assert_eq!(
        planned(&RequesterFacts::host("gateway")),
        Ok(PlannedOutcome::Allow)
    );
    assert_eq!(
        planned(&RequesterFacts::host("scheduler"))
            .unwrap_err()
            .to_data(),
        denied
    );
    assert_eq!(
        planned(&agent_of("lead-2", "lead", &[])),
        Ok(PlannedOutcome::Allow)
    );
    assert_eq!(
        planned(&agent_of("helper-2", "assistant", &[]))
            .unwrap_err()
            .to_data(),
        denied
    );
    assert_eq!(
        planned(&agent_of("analyst-1", "analyst", &[])),
        Ok(PlannedOutcome::Allow)
    );
    assert_eq!(
        planned(&agent_of("lead-3", "lead", &[("desk", "ops")])),
        Ok(PlannedOutcome::RequireApproval {
            approver_id: "security".to_owned()
        })
    );
    // A label key a requester selector reads is reserved like any other.
    assert!(policy.is_reserved_label("desk"));
}

#[test]
fn a_requester_selector_that_can_select_nobody_is_refused() {
    // A surface is the host's, and an identity, a profile or labels an
    // agent's; a human has none of them. A selector that names a member its
    // kind lacks, or names both a surface and an agent's member, selects
    // nobody: a configuration error, refused malformed at the selector,
    // never a silent no-op.
    let refused = [
        (
            json!({"kind": "agent", "surface": "console"}),
            "kind agent has no surface",
        ),
        (
            json!({"kind": "human", "surface": "console"}),
            "kind human has no surface",
        ),
        (
            json!({"kind": "host", "profile": ["lead"]}),
            "kind host has no profile",
        ),
        (
            json!({"kind": "host", "labels": {"desk": "ops"}}),
            "kind host has no labels",
        ),
        (
            json!({"kind": "human", "identity": ["lead-1"]}),
            "kind human has no identity",
        ),
        (
            json!({"surface": "console", "labels": {"desk": "ops"}}),
            "surface selects the host and labels an agent",
        ),
    ];
    for (selector, detail) in refused {
        let mut value = policy_json();
        value["approval"]["rules"][0]["requester"] = selector;
        let refusal = refusal_of(&value);
        assert_eq!(refusal.path, "approval.rules[0].requester", "{refusal}");
        let PolicyRefusalKind::Malformed { detail: shown } = &refusal.kind else {
            panic!("{refusal}");
        };
        assert!(shown.contains(detail), "{refusal}");
        assert!(
            shown.ends_with("the rule would match no requester"),
            "{refusal}"
        );
    }
    // A selector may name every member its kind has.
    let accepted = [
        json!({"kind": "host", "surface": "console"}),
        json!({"kind": "agent", "identity": ["lead-1"], "profile": ["lead"], "labels": {"desk": "ops"}}),
        json!({"surface": "gateway"}),
        json!({"profile": ["analyst"], "labels": {"desk": "ops"}}),
        json!({"kind": "human"}),
    ];
    for selector in accepted {
        let mut value = policy_json();
        value["approval"]["rules"][0]["requester"] = selector;
        assert!(IdentityCreationPolicy::parse(&value).is_ok(), "{value}");
    }
}

#[test]
fn tied_decisions_with_different_approvers_are_refused_before_anything_is_held() {
    // Two rules give equally strict decisions to different approvers. A
    // request under both has no one approver who may decide all of it, so it
    // is refused before anything is held, and the requester splits it; each
    // part alone is decided by its own rule's approver.
    let policy = parse(&json!({
        "enabled": true,
        "reserved_keys": {"labels": ["clearance"]},
        "authority_fields": {"clearance": {"values": ["basic", "elevated"]}},
        "approval": {"rules": [
            {"change": ["descriptive"],
             "outcome": {"kind": "require_approval", "approver": {"id": "owner"}}},
            {"change": ["authority"],
             "outcome": {"kind": "require_approval", "approver": {"id": "security"}}},
            {"change": ["create"], "target": {"profile": ["assistant"]},
             "outcome": {"kind": "require_approval", "approver": {"id": "owner"}}},
            {"change": ["create"], "target": {"profile": ["analyst"]},
             "outcome": {"kind": "require_approval", "approver": {"id": "security"}}}
        ]}
    }));
    let host = RequesterFacts::host("gateway");
    let entry = entry();
    let owner = PlannedOutcome::RequireApproval {
        approver_id: "owner".to_owned(),
    };
    let security = PlannedOutcome::RequireApproval {
        approver_id: "security".to_owned(),
    };
    let unavailable = json!({"kind": "approval_unavailable"});
    let rename = update_of(&entry, json!({"display_name": "Helper two"}));
    let planned = update_with(&policy, &host, &entry, &rename).expect("a rename");
    assert_eq!(planned.plan.outcome, owner);
    let elevate = update_of(
        &entry,
        json!({"authority_fields": {"clearance": "elevated"}}),
    );
    let planned = update_with(&policy, &host, &entry, &elevate).expect("an authority change");
    assert_eq!(planned.plan.outcome, security);
    let both = update_of(
        &entry,
        json!({"display_name": "Helper two", "authority_fields": {"clearance": "elevated"}}),
    );
    let refusal = update_with(&policy, &host, &entry, &both)
        .expect_err("no one approver decides both changes");
    assert_eq!(refusal.to_data(), unavailable);

    // A create batch under two such rules is refused the same way.
    let mut scout = spec("scout");
    set_profile(&mut scout, "analyst");
    let planned = create(&policy, &host, &[spec("helper")]).expect("one identity");
    assert_eq!(planned.plan.outcome, owner);
    let planned = create(&policy, &host, &[scout.clone()]).expect("one identity");
    assert_eq!(planned.plan.outcome, security);
    let refusal =
        create(&policy, &host, &[spec("helper"), scout]).expect_err("two approvers, one batch");
    assert_eq!(refusal.to_data(), unavailable);
}

#[test]
fn a_review_plans_one_reviewer_who_is_not_the_requester() {
    let review = |reviewer: &str| {
        let mut outcome = json!({"kind": "review", "approver": {"id": "owner"}, "max_rounds": 2});
        outcome["reviewer"] = json!(reviewer);
        outcome
    };
    let policy = parse(&json!({
        "enabled": true,
        "approval": {"rules": [
            {"change": ["descriptive", "capability"], "outcome": review("reviewer-1")},
            {"change": ["remove"], "outcome": review("reviewer-1")},
            {"change": ["remove"], "outcome": review("reviewer-2")}
        ]}
    }));
    let host = RequesterFacts::host("gateway");
    let entry = entry();
    let reviewed = PlannedOutcome::Review {
        approver_id: "owner".to_owned(),
        reviewer: "reviewer-1".to_owned(),
        max_rounds: 2,
    };
    // The rule's reviewer decides, and its approver takes an escalation.
    let rename = update_of(&entry, json!({"display_name": "Helper two"}));
    let planned = update_with(&policy, &host, &entry, &rename).expect("a review");
    assert_eq!(planned.plan.outcome, reviewed);
    assert!(planned.plan.holds_decision());
    // Equally strict reviews that name the same reviewer are one review.
    let both = update_of(
        &entry,
        json!({"display_name": "Helper two", "addressability": "internal_only"}),
    );
    let planned = update_with(&policy, &host, &entry, &both).expect("one review of both changes");
    assert_eq!(planned.plan.outcome, reviewed);
    // The requester never reviews its own request: when it is the reviewer,
    // nobody is left to decide, and nothing is held.
    let refusal = update_with(
        &policy,
        &agent_of("reviewer-1", "assistant", &[]),
        &entry,
        &rename,
    )
    .expect_err("the requester is the reviewer");
    assert_eq!(refusal.to_data(), json!({"kind": "approval_unavailable"}));
    // Equally strict reviews that name different reviewers leave no one
    // reviewer.
    let refusal = remove_identity_with(&policy, &host, &entry).expect_err("two reviewers");
    assert_eq!(refusal.to_data(), json!({"kind": "approval_unavailable"}));
}

#[test]
fn tied_notices_commit_with_the_first_approver() {
    // A notice holds nothing: equally strict notices that name different
    // approvers commit at once, and the first of them, in change class
    // order, then target order, then declaration order, names the approver.
    let notify = |approver: &str| json!({"kind": "notify", "approver": {"id": approver}});
    let policy = parse(&json!({
        "enabled": true,
        "approval": {"rules": [
            {"change": ["capability"], "outcome": notify("security")},
            {"change": ["descriptive"], "outcome": notify("owner")},
            {"change": ["remove"], "outcome": notify("owner")},
            {"change": ["remove"], "outcome": notify("security")}
        ]}
    }));
    let host = RequesterFacts::host("gateway");
    let entry = entry();
    let owner = PlannedOutcome::Notify {
        approver_id: "owner".to_owned(),
    };
    let both = update_of(
        &entry,
        json!({"display_name": "Helper two", "addressability": "internal_only"}),
    );
    let planned = update_with(&policy, &host, &entry, &both).expect("tied notices commit");
    assert_eq!(
        planned.plan.outcome, owner,
        "the descriptive change comes first in change class order"
    );
    assert_eq!(planned.plan.change, ChangeClass::Descriptive);
    assert!(!planned.plan.holds_decision());
    let removed = remove_identity_with(&policy, &host, &entry).expect("tied notices commit");
    assert_eq!(
        removed.plan.outcome, owner,
        "the first rule in declaration order"
    );
}

#[test]
fn the_request_digest_is_stable_across_input_order() {
    let mut first = spec("helper");
    set_profile(&mut first, "analyst");
    first.peers = vec!["lead-1".to_owned(), "analyst-1".to_owned()];
    set_labels(&mut first, &[("color", "blue"), ("shape", "round")]);
    set_context(&mut first, json!({"topic": "stars", "depth": 2}));
    let mut second = spec("scout");
    second.peers = vec!["helper".to_owned()];
    let forward = create(&policy(), &lead(), &[first.clone(), second.clone()]).expect("forward");
    first.peers = vec![
        "analyst-1".to_owned(),
        "lead-1".to_owned(),
        "lead-1".to_owned(),
    ];
    set_context(&mut first, json!({"depth": 2.0, "topic": "stars"}));
    let backward = create(&policy(), &lead(), &[second, first]).expect("backward");
    assert_eq!(forward.request, backward.request);
    assert_eq!(
        forward.request.request_digest().expect("a digest"),
        backward.request.request_digest().expect("a digest")
    );
    assert_eq!(render(&forward.request), render(&backward.request));
    assert_eq!(
        created_spec(&forward.request, "scout").peers["helper"],
        EdgeBasis::Rule {
            id: "analysts".to_owned()
        },
        "a peer the same request creates is selected by the rules like any identity"
    );
    let json = forward.request.to_json().expect("json");
    let read_back: IdentityRequest = serde_json::from_value(json).expect("the body reads back");
    assert_eq!(read_back, forward.request);
}

#[test]
fn the_console_updates_and_removes_any_identity_under_the_approval_rules() {
    let console = RequesterFacts::console();
    // Any identity, whatever its profile or how it came to be: the console
    // has no templates of its own, and only the approval rules decide.
    let entry = any_entry();
    let rename = update_of(&entry, json!({"display_name": "Helper two"}));
    let updated = update(&console, &entry, &rename).expect("the console updates any identity");
    assert_eq!(updated.plan.kind(), ApprovalKind::Notify);
    let removed = remove_identity(&console, &entry).expect("the console removes any identity");
    assert_eq!(removed.plan.kind(), ApprovalKind::Notify);

    // The rules select by the target's profile and authority labels,
    // whoever asks.
    let mut value = policy_json();
    value["approval"]["rules"]
        .as_array_mut()
        .expect("the rules")
        .extend([
            json!({"change": ["descriptive"], "target": {"labels": {"clearance": "elevated"}},
                   "outcome": {"kind": "require_approval", "approver": {"id": "security"}}}),
            json!({"change": ["remove"], "target": {"profile": ["lead"]},
                   "outcome": {"kind": "deny"}}),
        ]);
    let targeted = parse(&value);
    let elevated = entry_with("assistant", &[("clearance", "elevated")]);
    let rename_elevated = update_of(&elevated, json!({"display_name": "Helper two"}));
    let host = RequesterFacts::host("gateway");
    let lead_agent = lead();
    for requester in [&console, &host, &lead_agent] {
        let planned = update_with(&targeted, requester, &elevated, &rename_elevated)
            .expect("the update is planned");
        assert_eq!(
            planned.plan.outcome,
            PlannedOutcome::RequireApproval {
                approver_id: "security".to_owned()
            }
        );
    }
    let refusal = remove_identity_with(&targeted, &console, &entry_with("lead", &[]))
        .expect_err("removing a lead is denied");
    assert_eq!(
        refusal.to_data(),
        json!({"kind": "change_denied", "change": "remove"})
    );
    let removed = remove_identity_with(&targeted, &console, &entry_with("assistant", &[]))
        .expect("removing another profile's identity is planned");
    assert_eq!(removed.plan.kind(), ApprovalKind::Notify);
}

#[test]
fn update_and_remove_check_self_change_spec_digest_and_change_classes() {
    let entry = entry();
    let rename = update_of(&entry, json!({"display_name": "Helper two"}));
    // Any agent's update goes to the rules of its change classes: who
    // created the identity is provenance, and grants or refuses nothing.
    let other =
        update(&agent("other-1"), &entry, &rename).expect("another agent's update is planned");
    assert_eq!(other.plan.change, ChangeClass::Descriptive);
    assert_eq!(other.plan.kind(), ApprovalKind::Notify);
    let normalized = update(&lead(), &entry, &rename).expect("an update");
    assert_eq!(normalized.plan.kind(), ApprovalKind::Notify);
    assert_eq!(normalized.request.decision_kind(), "identity.update");

    // An agent never changes its own identity.
    assert_eq!(
        update(&agent("helper"), &entry, &rename).unwrap_err().kind,
        IdentityRefusalKind::SelfChangeRefused
    );
    // A change names the spec its requester read by its digest, since the
    // identity's owner may keep no revision; a store-owned identity's
    // revision is reported beside it.
    let refusal = update(&lead(), &entry, &stale_update(&rename)).unwrap_err();
    let stale_spec = json!({
        "kind": "stale_spec",
        "current_digest": spec_digest_of(&entry),
        "current_revision": 3,
        "field": "expected_spec_digest"
    });
    assert_eq!(refusal.to_data(), stale_spec);
    assert_eq!(
        stale_removal(&lead(), &entry).unwrap_err().to_data(),
        stale_spec
    );
    // A roster-owned identity has no revision: its stale spec names the
    // digest alone.
    let roster = IdentityEntryFacts {
        revision: None,
        ..entry.clone()
    };
    let roster_stale = json!({
        "kind": "stale_spec",
        "current_digest": spec_digest_of(&roster),
        "field": "expected_spec_digest"
    });
    let roster_rename = update_of(&roster, json!({"display_name": "Helper two"}));
    assert_eq!(
        update(&lead(), &roster, &stale_update(&roster_rename))
            .unwrap_err()
            .to_data(),
        roster_stale
    );
    assert_eq!(
        stale_removal(&lead(), &roster).unwrap_err().to_data(),
        roster_stale
    );
    assert_eq!(
        update(&lead(), &entry, &update_of(&entry, json!({})))
            .unwrap_err()
            .kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty
        }
    );
    let conflicting = update_of(
        &entry,
        json!({"peers_add": ["core-1"], "peers_remove": ["core-1"]}),
    );
    assert_eq!(
        update(&lead(), &entry, &conflicting).unwrap_err().kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Conflicting
        }
    );

    // Capability: how the identity runs and who reaches it.
    let capability = update_of(
        &entry,
        json!({"addressability": "internal_only", "peers_add": ["analyst-1"]}),
    );
    let normalized = update(&lead(), &entry, &capability).expect("capability and peers");
    assert_eq!(normalized.plan.kind(), ApprovalKind::RequireApproval);
    let RequestAction::Update(updated) = &normalized.request.action else {
        panic!("an update");
    };
    assert_eq!(
        updated.change_classes(),
        BTreeSet::from([ChangeClass::Capability, ChangeClass::Peers])
    );
    assert_eq!(
        updated.peers_add["analyst-1"],
        EdgeBasis::Rule {
            id: "analysts".to_owned()
        }
    );

    // A profile change is the role migration: a change class of its own,
    // which records the profile it migrates from.
    let migration = update_of(&entry, json!({"profile": "analyst"}));
    assert_eq!(
        update(&lead(), &entry, &migration).unwrap_err().to_data(),
        json!({"kind": "change_denied", "change": "profile"}),
        "no rule lets an agent migrate a role, and the default denies"
    );
    let host = RequesterFacts::host("gateway");
    let migrated = update(&host, &entry, &migration).expect("the host migrates a role");
    assert_eq!(migrated.plan.kind(), ApprovalKind::Allow);
    assert_eq!(
        migrated.request.to_json().expect("the request body")["action"]["profile"],
        json!({"from": "assistant", "to": "analyst"})
    );
    let ghost = update_of(&entry, json!({"profile": "ghost"}));
    assert_eq!(
        update(&host, &entry, &ghost).unwrap_err().to_data(),
        json!({"kind": "profile_unknown", "profile": "ghost", "field": "set.profile"})
    );
    let current = update_of(&entry, json!({"profile": "assistant"}));
    assert_eq!(
        update(&host, &entry, &current).unwrap_err().kind,
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty
        },
        "the current profile changes nothing"
    );

    // Any agent but the identity itself may ask for its removal.
    for requester in [lead(), agent("other-1")] {
        let removed = remove_identity(&requester, &entry).expect("the removal is planned");
        assert_eq!(removed.plan.kind(), ApprovalKind::Notify);
        assert_eq!(removed.request.decision_kind(), "identity.remove");
    }
    assert_eq!(
        remove_identity(&agent("helper"), &entry).unwrap_err().kind,
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

Identity 1 of 1: "helper"
Profile: "assistant"
Addressability: addressable
Runtime mode: none
Backend: none
Binding: none
Placement: none
Peers (2):
- "core-1" basis rule "sensitive"
- "lead-1" basis rule "leads"
Labels (2):
- "clearance" = "elevated"
- "color" = "blue"

Consequences (2):
- "helper" peer "core-1" needs approval under rule "sensitive" because "a core team member"
- "helper" authority field "clearance" = "elevated" becomes authority

Host consequences (0):

Text of identity 1 of 1: "helper"
Display name (1 line):
> Helper
Additional instructions: 1
Additional instruction 1 of 1 (2 lines):
> Read the notes.
> Report daily.
Initial message: none
Context (4 lines):
> {
>   "depth": 2,
>   "topic": "stars"
> }
"#;

const GOLDEN_RUNTIME_CREATE: &str = r#"Identity request: render-v1
Action: create
Requester: host "gateway"
Proposer: none
Source evidence: none
Identities: 1

Identity 1 of 1: "scout"
Profile: "analyst"
Addressability: internal-only
Runtime mode: turn-driven
Backend: external
Binding (8 lines):
> {
>   "address": "tcp://127.0.0.1:7000",
>   "identity": {
>     "kind": "ed25519\_public\_key",
>     "public\_key": "ed25519:AAAA"
>   },
>   "kind": "external"
> }
Placement: "host-7"
Peers (0):
Labels (0):

Consequences (0):

Host consequences (0):

Text of identity 1 of 1: "scout"
Display name: none
Additional instructions: 0
Initial message blocks (6 lines):
> \[
>   {
>     "text": "Begin.",
>     "type": "text"
>   }
> \]
Context: none
"#;

const GOLDEN_UPDATE: &str = r#"Identity request: render-v1
Action: update
Requester: agent "lead-1"
Proposer: none
Source evidence: none
Identity: "helper"
Changes: descriptive capability peers profile

Update of: "helper"
Profile: "assistant" to "analyst"
Addressability: internal-only
Runtime mode: unchanged
Peers added (1):
- "analyst-1" basis rule "analysts"
Peers removed (1):
- "core-1"
Authority fields: unchanged
Descriptive labels: unchanged

Consequences (0):

Host consequences (0):

Text of: "helper"
Display name: unchanged
Additional instructions: 1
Additional instruction 1 of 1 (1 line):
> Find better sources.
Context: unchanged
"#;

const GOLDEN_REMOVE: &str = r#"Identity request: render-v1
Action: remove
Requester: host "console"
Proposer: "op-proposal-7"
Source attestor: "adapter-1"
Source subject: "user-9"
Source reference: "event:42"
Source observed at ms: 1730000000000
Identity: "helper"

Consequences (0):

Host consequences (1):
- "Removes a helper that holds private notes"
"#;

#[test]
fn render_v1_is_the_pinned_canonical_text() {
    assert_eq!(render(&golden_create()).as_str(), GOLDEN_CREATE);
    assert_eq!(
        render(&golden_runtime_create()).as_str(),
        GOLDEN_RUNTIME_CREATE
    );
    assert_eq!(render(&golden_update()).as_str(), GOLDEN_UPDATE);
    let hook = |_: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        Ok(lines(&["Removes a helper that holds private notes"]))
    };
    let removal = render_v1(&golden_remove(), Some(&hook)).expect("a rendering");
    assert_eq!(removal.as_str(), GOLDEN_REMOVE);
    assert_eq!(
        removal.subject_digest(),
        Sha256Digest::of(GOLDEN_REMOVE.as_bytes())
    );
    for golden in [
        GOLDEN_CREATE,
        GOLDEN_RUNTIME_CREATE,
        GOLDEN_UPDATE,
        GOLDEN_REMOVE,
    ] {
        assert!(StructuredCard::parse(golden).is_ok());
    }
}

/// A rendering names a person by their name, the id their principal key
/// carries, as an agent's record names who asked for it: the key itself is
/// never shown, for the requester or for a person a host names as the
/// proposer. Any other proposer text is shown as it was written, and the
/// request digest still binds the keys.
#[test]
fn a_rendering_names_a_person_and_never_shows_a_principal_key() {
    const CAROL: &str = r#"principal:["human","example-domain","carol"]"#;
    const DAVE: &str = r#"principal:["human","example-domain","dave"]"#;
    let asked_by_a_person = IdentityRequest {
        requester: Requester::Human {
            principal: CAROL.to_owned(),
        },
        proposer: None,
        ..golden_remove()
    };
    let shown = render(&asked_by_a_person);
    assert!(
        shown
            .as_str()
            .contains("\nRequester: human \"carol\"\nProposer: none\n"),
        "{}",
        shown.as_str()
    );
    assert!(!shown.as_str().contains("principal:"), "{}", shown.as_str());

    let proposed_by_a_person = IdentityRequest {
        proposer: Some(DAVE.to_owned()),
        ..golden_remove()
    };
    let shown = render(&proposed_by_a_person);
    assert!(
        shown
            .as_str()
            .contains("\nRequester: host \"console\"\nProposer: \"dave\"\n"),
        "{}",
        shown.as_str()
    );
    assert!(!shown.as_str().contains("principal:"), "{}", shown.as_str());

    // Another person's request is another request, whatever is shown.
    let asked_by_another = IdentityRequest {
        requester: Requester::Human {
            principal: DAVE.to_owned(),
        },
        ..asked_by_a_person.clone()
    };
    assert_ne!(
        asked_by_a_person.request_digest().expect("a digest"),
        asked_by_another.request_digest().expect("a digest")
    );
    assert_ne!(
        render(&asked_by_a_person).subject_digest(),
        render(&asked_by_another).subject_digest()
    );

    // A proposer that is no person's key is shown as written.
    assert!(
        render(&golden_remove())
            .as_str()
            .contains("\nProposer: \"op-proposal-7\"\n")
    );
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

/// The characters render-v1 writes as `\u{XXXX}`, as inclusive ranges of
/// code points. Changing the set is render-v2.
const HIDDEN_V1: [(u32, u32); 46] = [
    (0x0000, 0x001F),
    (0x007F, 0x00A0),
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x0600, 0x0605),
    (0x061C, 0x061C),
    (0x06DD, 0x06DD),
    (0x070F, 0x070F),
    (0x0890, 0x0891),
    (0x08E2, 0x08E2),
    (0x115F, 0x1160),
    (0x1680, 0x1680),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x2000, 0x200F),
    (0x2028, 0x202F),
    (0x205F, 0x206F),
    (0x2800, 0x2800),
    (0x3000, 0x3000),
    (0x3164, 0x3164),
    (0xE000, 0xF8FF),
    (0xFDD0, 0xFDEF),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFFB),
    (0xFFFE, 0xFFFF),
    (0x110BD, 0x110BD),
    (0x110CD, 0x110CD),
    (0x13430, 0x1343F),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0x1FFFE, 0x1FFFF),
    (0x2FFFE, 0x2FFFF),
    (0x3FFFE, 0x3FFFF),
    (0x4FFFE, 0x4FFFF),
    (0x5FFFE, 0x5FFFF),
    (0x6FFFE, 0x6FFFF),
    (0x7FFFE, 0x7FFFF),
    (0x8FFFE, 0x8FFFF),
    (0x9FFFE, 0x9FFFF),
    (0xAFFFE, 0xAFFFF),
    (0xBFFFE, 0xBFFFF),
    (0xCFFFE, 0xCFFFF),
    (0xDFFFE, 0xE0FFF),
    (0xEFFFE, 0x10_FFFF),
];

#[test]
fn the_escape_sets_are_frozen_for_render_v1() {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for character in (0..=0x10_FFFF_u32).filter_map(char::from_u32) {
        if !is_hidden(character) {
            continue;
        }
        let code = u32::from(character);
        match ranges.last() {
            Some(&(start, end)) if end + 1 == code => {
                let last = ranges.len() - 1;
                ranges[last] = (start, code);
            }
            _ => ranges.push((code, code)),
        }
    }
    assert_eq!(ranges, HIDDEN_V1);
    let markup: String = (0..=0x10_FFFF_u32)
        .filter_map(char::from_u32)
        .filter(|character| is_markup(*character))
        .collect();
    assert_eq!(markup, "&*<>[]_`|~");
}

#[test]
fn a_rendering_past_its_limit_is_refused() {
    let mut request = golden_create();
    created_spec_mut(&mut request, "helper")
        .spec
        .additional_instructions = vec!["x".repeat(RENDERING_MAX_BYTES)];
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
    // 140000 quotes: about 140 KB of rendering, but about 280 KB of JSON.
    let mut quotes = spec("helper");
    quotes.spec.additional_instructions = vec!["\"".repeat(140_000)];
    let refusal = create(&policy(), &lead(), &[quotes.clone()]).expect_err("a body past its limit");
    assert_eq!(
        refusal.kind,
        IdentityRefusalKind::TextTooLarge {
            limit: REQUEST_BODY_MAX_BYTES as u64
        }
    );
    assert_eq!(refusal.field.as_deref(), Some("request"));
    // The same length of plain text fits: the body was the bound, not the
    // text.
    quotes.spec.additional_instructions = vec!["x".repeat(140_000)];
    let fits = create(&policy(), &lead(), &[quotes]).expect("a body within its limit");
    assert!(render_v1(&fits.request, None).is_ok());
}

// ---------------------------------------------------------------------------
// The contract's rendering rows, as pure render and digest checks
// ---------------------------------------------------------------------------

#[test]
fn an_escaped_rendering_of_an_unchanged_request_approves() {
    let mut request = golden_create();
    {
        let spec = &mut created_spec_mut(&mut request, "helper").spec;
        spec.display_name = Some(display_name_of(
            "||Approve me|| [now](https://example.invalid)",
        ));
        spec.additional_instructions = vec![
            "<b>bold</b> & *loud* ~gone~ _quiet_".to_owned(),
            "line one\u{202E}reversed\nzero\u{200B}width\n```code```".to_owned(),
        ];
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
    let spec = &created_spec(&request, "helper").spec;
    assert_eq!(
        texts[labels::DISPLAY_NAME].display_lines(),
        [
            "Display name (1 line):",
            "> \\|\\|Approve me\\|\\| \\[now\\](https://example.invalid)"
        ]
    );
    let first = format!("{} 1 of 2", labels::ADDITIONAL_INSTRUCTION);
    let second = format!("{} 2 of 2", labels::ADDITIONAL_INSTRUCTION);
    for (label, expected) in [
        (
            labels::DISPLAY_NAME,
            spec.display_name.as_ref().map(DisplayName::as_str),
        ),
        (
            first.as_str(),
            spec.additional_instructions.first().map(String::as_str),
        ),
        (
            second.as_str(),
            spec.additional_instructions.get(1).map(String::as_str),
        ),
    ] {
        assert_eq!(
            texts[label].decoded_text_not_for_display().as_deref(),
            expected,
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
    created_spec_mut(&mut request, "helper")
        .spec
        .additional_instructions = vec![
        (0..120)
            .map(|line| format!("Step {line}: read the café notes ✓ and write 漢字 summaries."))
            .collect::<Vec<_>>()
            .join("\n"),
    ];
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
    created_spec_mut(&mut base, "helper").spec.initial_message =
        Some(ContentInput::Text("Start.".to_owned()));
    let old = render(&base);
    let old_tap = old.subject_digest();

    type RequestChange = fn(&mut IdentityRequest);
    fn change(change: RequestChange) -> RequestChange {
        change
    }
    fn created_of(request: &mut IdentityRequest) -> &mut CreatedSpec {
        created_spec_mut(request, "helper")
    }
    fn spec_of(request: &mut IdentityRequest) -> &mut DurableAgentSpec {
        &mut created_of(request).spec
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
        change(|request| spec_of(request).identity = identity_of("helper2")),
        change(|request| spec_of(request).profile = ProfileName::from("analyst")),
        change(|request| spec_of(request).addressability = AgentAddressability::InternalOnly),
        change(|request| spec_of(request).runtime_mode_override = Some(MobRuntimeMode::TurnDriven)),
        change(|request| {
            spec_of(request).runtime_mode_override = Some(MobRuntimeMode::AutonomousHost);
        }),
        change(|request| spec_of(request).backend = Some(MobBackendKind::Session)),
        change(|request| spec_of(request).backend = Some(MobBackendKind::External)),
        change(|request| {
            spec_of(request).binding = Some(
                serde_json::from_value(json!({"kind": "session"})).expect("a runtime binding"),
            );
        }),
        change(|request| spec_of(request).placement = Some(WireHostRef("host-7".to_owned()))),
        change(|request| spec_of(request).display_name = Some(display_name_of("Helpers"))),
        change(|request| spec_of(request).display_name = None),
        change(|request| {
            spec_of(request).additional_instructions =
                vec!["Read the notes.\nReport daily.\n".to_owned()];
        }),
        change(|request| {
            spec_of(request).additional_instructions =
                vec!["Read the notes. Report daily.".to_owned()];
        }),
        change(|request| {
            spec_of(request).additional_instructions =
                vec!["Read the notes.".to_owned(), "Report daily.".to_owned()];
        }),
        change(|request| spec_of(request).additional_instructions = Vec::new()),
        change(|request| spec_of(request).context = None),
        change(|request| {
            spec_of(request).context = Some(json!({"topic": "stars", "depth": 3}));
        }),
        change(|request| {
            spec_of(request).context = Some(json!({"topic": "stars", "depth": "2"}));
        }),
        change(|request| spec_of(request).context = Some(json!("stars"))),
        change(|request| {
            spec_of(request)
                .labels
                .insert("color".to_owned(), "red".to_owned());
        }),
        change(|request| {
            spec_of(request)
                .labels
                .insert("shape".to_owned(), "round".to_owned());
        }),
        change(|request| {
            spec_of(request)
                .labels
                .insert("clearance".to_owned(), "basic".to_owned());
        }),
        change(|request| {
            created_of(request).peers.insert(
                "analyst-1".to_owned(),
                EdgeBasis::Rule {
                    id: "analysts".to_owned(),
                },
            );
        }),
        change(|request| {
            created_of(request).peers.insert(
                "lead-1".to_owned(),
                EdgeBasis::Rule {
                    id: "default".to_owned(),
                },
            );
        }),
        change(|request| spec_of(request).initial_message = None),
        change(|request| {
            spec_of(request).initial_message = Some(ContentInput::Text(String::new()));
        }),
        change(|request| {
            spec_of(request).initial_message = Some(ContentInput::Text("Start!".to_owned()));
        }),
        change(|request| {
            spec_of(request).initial_message =
                Some(ContentInput::Blocks(vec![ContentBlock::Text {
                    text: "Start.".to_owned(),
                }]));
        }),
        change(|request| {
            request.consequences.insert(Consequence {
                identity: "helper".to_owned(),
                effect: ConsequenceEffect::PeerNeedsApproval {
                    peer: "analyst-1".to_owned(),
                    rule: None,
                    reason: None,
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
fn a_changed_update_or_remove_field_cannot_be_approved_by_an_old_tap() {
    type Change = fn(&mut IdentityRequest);
    fn change(change: Change) -> Change {
        change
    }
    fn migration(from: &str, to: &str) -> Option<ProfileChange> {
        Some(ProfileChange {
            from: ProfileName::from(from),
            to: ProfileName::from(to),
        })
    }
    let updates = [
        change(|request| updated_mut(request).identity = "other".to_owned()),
        change(|request| updated_mut(request).profile = None),
        change(|request| updated_mut(request).profile = migration("assistant", "lead")),
        change(|request| updated_mut(request).profile = migration("lead", "analyst")),
        change(|request| updated_mut(request).addressability = None),
        change(|request| {
            updated_mut(request).addressability = Some(AgentAddressability::Addressable);
        }),
        change(|request| {
            updated_mut(request).runtime_mode_override = Some(MobRuntimeMode::TurnDriven);
        }),
        change(|request| updated_mut(request).display_name = Some(display_name_of("Helper"))),
        change(|request| updated_mut(request).additional_instructions = None),
        change(|request| updated_mut(request).additional_instructions = Some(Vec::new())),
        change(|request| {
            updated_mut(request).additional_instructions = Some(vec!["Read.".to_owned()]);
        }),
        change(|request| updated_mut(request).context = Some(json!({}))),
        change(|request| updated_mut(request).descriptive_labels = Some(BTreeMap::new())),
        change(|request| updated_mut(request).authority_fields = Some(BTreeMap::new())),
        change(|request| updated_mut(request).peers_add.clear()),
        change(|request| {
            updated_mut(request).peers_add.insert(
                "analyst-1".to_owned(),
                EdgeBasis::Rule {
                    id: "leads".to_owned(),
                },
            );
        }),
        change(|request| updated_mut(request).peers_remove.clear()),
        change(|request| {
            updated_mut(request)
                .peers_remove
                .insert("stranger".to_owned());
        }),
    ];
    let removes = [change(|request| {
        request.action = RequestAction::Remove {
            identity: "other".to_owned(),
            expected_spec_digest: helper_spec_digest(),
        };
    })];
    for (base, changes) in [
        (golden_update(), updates.as_slice()),
        (golden_remove(), removes.as_slice()),
    ] {
        let old_tap = render(&base).subject_digest();
        let mut digests = BTreeSet::from([old_tap.clone()]);
        for (index, apply) in changes.iter().enumerate() {
            let mut changed = base.clone();
            apply(&mut changed);
            assert_ne!(changed, base, "change {index} changes the request");
            let rendering = render(&changed);
            let parts = rendering.canonical_parts(None);
            assert_eq!(
                decide(&rendering, &old_tap, None, &parts),
                Err(IdentityRefusalKind::SubjectChanged),
                "change {index}"
            );
            assert!(
                digests.insert(rendering.subject_digest()),
                "change {index} renders a subject of its own"
            );
        }
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
    scout.spec.identity = identity_of("scout");
    scout.spec.display_name = Some(display_name_of("Scout *one*"));
    scout.spec.labels = BTreeMap::from([("focus_area".to_owned(), "sky".to_owned())]);
    scout.spec.additional_instructions = vec!["Look around.\n\nReport [findings].".to_owned()];
    scout.spec.initial_message = Some(ContentInput::Text("Begin.".to_owned()));
    scout.spec.context = None;
    scout.spec.runtime_mode_override = Some(MobRuntimeMode::TurnDriven);
    scout.spec.placement = Some(WireHostRef("host-7".to_owned()));
    let RequestAction::Create { identities } = &mut request.action else {
        panic!("a create request");
    };
    identities.push(scout.clone());
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
            assert_eq!(value.quoted(), Some("\"scout\""));
            assert_eq!(value.decoded_not_for_display().as_deref(), Some("scout"));
        }
        other => panic!("{other:?}"),
    }
    let field = |block: &CardBlock, wanted: &str| -> CardValue {
        block
            .entries()
            .iter()
            .find_map(|entry| match entry {
                CardEntry::Field { label, value } if label == wanted => Some(value.clone()),
                _ => None,
            })
            .expect("the field")
    };
    assert_eq!(
        field(&blocks[2], labels::RUNTIME_MODE).word(),
        Some("turn-driven")
    );
    assert_eq!(
        field(&blocks[2], labels::PLACEMENT).quoted(),
        Some("\"host-7\"")
    );
    let scout_labels = blocks[2]
        .entries()
        .iter()
        .find_map(|entry| match entry {
            CardEntry::List { label, items } if label == labels::LABELS => Some(items),
            _ => None,
        })
        .expect("the scout's labels");
    let shown: Vec<String> = scout_labels.iter().map(ToString::to_string).collect();
    assert_eq!(shown, ["\"focus\\_area\" = \"sky\""]);
    let decoded: Vec<String> = scout_labels[0]
        .segments()
        .iter()
        .filter_map(|segment| match segment {
            ValueSegment::Quoted(text) => unquote(text).map(|(decoded, _)| decoded),
            ValueSegment::Word(_) => None,
        })
        .collect();
    assert_eq!(decoded, ["focus_area", "sky"]);

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
    let instruction = format!("{} 1 of 1", labels::ADDITIONAL_INSTRUCTION);
    let scout_text = blocks[6]
        .entries()
        .iter()
        .find(|entry| entry.label() == instruction)
        .expect("the scout's instruction");
    match scout_text {
        CardEntry::Text { lines, .. } => {
            assert_eq!(lines, &["Look around.", "", "Report \\[findings\\]."]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        scout_text.decoded_text_not_for_display().as_ref(),
        scout.spec.additional_instructions.first()
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
        let spec = &mut created_spec_mut(&mut request, "helper").spec;
        spec.display_name = Some(display_name_of("x\u{202E}y"));
        spec.additional_instructions = vec!["a\u{E0041}b\u{200B}c".to_owned()];
        spec.labels = BTreeMap::from([
            (
                "note".to_owned(),
                "blue\u{2028}\u{2028}Host consequences (0):".to_owned(),
            ),
            ("multi".to_owned(), "a\nb".to_owned()),
        ]);
        spec.placement = Some(WireHostRef("host\u{202E}".to_owned()));
    }
    request.consequences.insert(Consequence {
        identity: "helper".to_owned(),
        effect: ConsequenceEffect::PeerNeedsApproval {
            peer: "shell\u{202E}".to_owned(),
            rule: Some("sensitive".to_owned()),
            reason: None,
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
        "\"host\\u{202E}\"",
        "\"note\" = \"blue\\u{2028}\\u{2028}Host consequences (0):\"",
        "\"multi\" = \"a\\u{000A}b\"",
        "\"helper\" peer \"shell\\u{202E}\" needs approval under rule \"sensitive\"",
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
    assert!(StructuredCard::parse(GOLDEN_CREATE).is_ok());
    let golden = GOLDEN_CREATE.to_owned();
    let tampered = [
        golden.replace("\"assistant\"", "\"assist_ant\""),
        golden.replace(
            "Addressability: addressable",
            "Addressability: internal_only",
        ),
        golden.replace("Peers (2):", "Peers (3):"),
        golden.replace("Peers (2):", "Peers (02):"),
        golden.replace("Display name (1 line):", "Display name (1 lines):"),
        golden.replace("render-v1", "render-v2"),
        golden.replace("> Helper", "> Hel\u{202E}per"),
        golden.replace("> Helper", "> \\u{0041}"),
        golden.replace("> Helper", "> "),
        golden.replace("Identities: 1\n", "Identities: 1\n\n"),
        golden.replace("Initial message: none", "Initial message:  none"),
        golden.replace("Initial message: none", "Initial message: \"none"),
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

/// One refusal of every kind, in the order of the design's table. The match
/// below stops compiling when a kind is added without a sample here.
fn every_refusal_kind() -> Vec<IdentityRefusalKind> {
    let kinds = vec![
        IdentityRefusalKind::ProfileUnknown {
            profile: "p".to_owned(),
        },
        IdentityRefusalKind::ProfileNotAllowed {
            profile: "p".to_owned(),
        },
        IdentityRefusalKind::PrivilegedField {
            field: "backend".to_owned(),
        },
        IdentityRefusalKind::GrantRevoked,
        IdentityRefusalKind::ReservedKey {
            key: "k".to_owned(),
        },
        IdentityRefusalKind::AuthorityFieldNeedsApproval {
            key: "k".to_owned(),
        },
        IdentityRefusalKind::AuthorityFieldInvalid {
            key: "k".to_owned(),
        },
        IdentityRefusalKind::TextTooLarge { limit: 1 },
        IdentityRefusalKind::ConsequencesUnavailable,
        IdentityRefusalKind::InvalidName,
        IdentityRefusalKind::NameTaken {
            by: NameTakenBy::Identity,
        },
        IdentityRefusalKind::PeerNotAllowed {
            identity: "p".to_owned(),
        },
        IdentityRefusalKind::PeerUnknown,
        IdentityRefusalKind::BatchTooLarge { max: 256 },
        IdentityRefusalKind::ValidationRefused {
            reason: "r".to_owned(),
        },
        IdentityRefusalKind::ChangeDenied {
            change: ChangeClass::Create,
        },
        IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty,
        },
        IdentityRefusalKind::Unsupported {
            feature: "f".to_owned(),
        },
        IdentityRefusalKind::IdentityUnknown,
        IdentityRefusalKind::SelfChangeRefused,
        IdentityRefusalKind::PrivateRequester,
        IdentityRefusalKind::StaleSpec {
            current_digest: Sha256Digest::of(b"s"),
            current_revision: Some(1),
        },
        IdentityRefusalKind::OperationReused,
        IdentityRefusalKind::OwnerReadOnly,
        IdentityRefusalKind::NotAdoptable {
            reason: NotAdoptableReason::HostedOnFirstUse,
        },
        IdentityRefusalKind::NotArchived,
        IdentityRefusalKind::DeclaredApplyOpen,
        IdentityRefusalKind::AdmissionUnsettled {
            correlation: "c".to_owned(),
        },
        IdentityRefusalKind::PredecessorMismatch,
        IdentityRefusalKind::ImportRefused,
        IdentityRefusalKind::PurgeRefused {
            reason: "changes_not_held".to_owned(),
        },
        IdentityRefusalKind::PreparationNotPending,
        IdentityRefusalKind::PreparationMismatch,
        IdentityRefusalKind::ConstructionOpen,
        IdentityRefusalKind::StaleAttempt { current: None },
        IdentityRefusalKind::NotConstructed,
        IdentityRefusalKind::ConstructionRequired,
        IdentityRefusalKind::ActivationPending { attempt: 1 },
        IdentityRefusalKind::ApprovalUnavailable,
        IdentityRefusalKind::DecisionOpen {
            decision_id: "d".to_owned(),
        },
        IdentityRefusalKind::SubjectWidened,
        IdentityRefusalKind::DecisionClosed {
            state: "expired".to_owned(),
        },
        IdentityRefusalKind::DecisionNotFound,
        IdentityRefusalKind::EvidenceRequired,
        IdentityRefusalKind::SubjectChanged,
        IdentityRefusalKind::PresentationIncomplete,
        IdentityRefusalKind::RequesterCannotDecide,
        IdentityRefusalKind::ApproverNotEligible,
        IdentityRefusalKind::DecidedBeforePresentation,
        IdentityRefusalKind::ChangesHeld,
        IdentityRefusalKind::HostPreparationPending,
        IdentityRefusalKind::CandidateLaunch,
        IdentityRefusalKind::IdentityMember,
        IdentityRefusalKind::StorageUnsupported,
        IdentityRefusalKind::MobNotRunning,
        IdentityRefusalKind::Purged,
        IdentityRefusalKind::ConstructionRefused {
            kind: "controller_route_refused".to_owned(),
        },
    ];
    for kind in &kinds {
        match kind {
            IdentityRefusalKind::ProfileUnknown { .. }
            | IdentityRefusalKind::ProfileNotAllowed { .. }
            | IdentityRefusalKind::PrivilegedField { .. }
            | IdentityRefusalKind::GrantRevoked
            | IdentityRefusalKind::ReservedKey { .. }
            | IdentityRefusalKind::AuthorityFieldNeedsApproval { .. }
            | IdentityRefusalKind::AuthorityFieldInvalid { .. }
            | IdentityRefusalKind::TextTooLarge { .. }
            | IdentityRefusalKind::ConsequencesUnavailable
            | IdentityRefusalKind::InvalidName
            | IdentityRefusalKind::NameTaken { .. }
            | IdentityRefusalKind::PeerNotAllowed { .. }
            | IdentityRefusalKind::PeerUnknown
            | IdentityRefusalKind::BatchTooLarge { .. }
            | IdentityRefusalKind::ValidationRefused { .. }
            | IdentityRefusalKind::ChangeDenied { .. }
            | IdentityRefusalKind::InvalidRequest { .. }
            | IdentityRefusalKind::Unsupported { .. }
            | IdentityRefusalKind::IdentityUnknown
            | IdentityRefusalKind::SelfChangeRefused
            | IdentityRefusalKind::PrivateRequester
            | IdentityRefusalKind::StaleSpec { .. }
            | IdentityRefusalKind::OperationReused
            | IdentityRefusalKind::OwnerReadOnly
            | IdentityRefusalKind::NotAdoptable { .. }
            | IdentityRefusalKind::NotArchived
            | IdentityRefusalKind::DeclaredApplyOpen
            | IdentityRefusalKind::AdmissionUnsettled { .. }
            | IdentityRefusalKind::PredecessorMismatch
            | IdentityRefusalKind::ImportRefused
            | IdentityRefusalKind::PurgeRefused { .. }
            | IdentityRefusalKind::PreparationNotPending
            | IdentityRefusalKind::PreparationMismatch
            | IdentityRefusalKind::ConstructionOpen
            | IdentityRefusalKind::StaleAttempt { .. }
            | IdentityRefusalKind::NotConstructed
            | IdentityRefusalKind::ConstructionRequired
            | IdentityRefusalKind::ActivationPending { .. }
            | IdentityRefusalKind::ApprovalUnavailable
            | IdentityRefusalKind::DecisionOpen { .. }
            | IdentityRefusalKind::SubjectWidened
            | IdentityRefusalKind::DecisionClosed { .. }
            | IdentityRefusalKind::DecisionNotFound
            | IdentityRefusalKind::EvidenceRequired
            | IdentityRefusalKind::SubjectChanged
            | IdentityRefusalKind::PresentationIncomplete
            | IdentityRefusalKind::RequesterCannotDecide
            | IdentityRefusalKind::ApproverNotEligible
            | IdentityRefusalKind::DecidedBeforePresentation
            | IdentityRefusalKind::ChangesHeld
            | IdentityRefusalKind::HostPreparationPending
            | IdentityRefusalKind::CandidateLaunch
            | IdentityRefusalKind::IdentityMember
            | IdentityRefusalKind::StorageUnsupported
            | IdentityRefusalKind::MobNotRunning
            | IdentityRefusalKind::Purged
            | IdentityRefusalKind::ConstructionRefused { .. } => {}
        }
    }
    kinds
}

#[test]
fn every_refusal_token_is_the_closed_vocabulary() {
    let kinds = every_refusal_kind();
    let tokens: Vec<&str> = kinds.iter().map(IdentityRefusalKind::token).collect();
    assert_eq!(
        tokens,
        [
            // Profile.
            "profile_unknown",
            "profile_not_allowed",
            "privileged_field",
            "grant_revoked",
            // Request data.
            "reserved_key",
            "authority_field_needs_approval",
            "authority_field_invalid",
            "text_too_large",
            "consequences_unavailable",
            // Name.
            "invalid_name",
            "name_taken",
            // Peers.
            "peer_not_allowed",
            "peer_unknown",
            // Policy.
            "batch_too_large",
            "validation_refused",
            "change_denied",
            "invalid_request",
            "unsupported",
            // Identities and operations.
            "identity_unknown",
            "self_change_refused",
            "private_requester",
            "stale_spec",
            "operation_reused",
            "owner_read_only",
            "not_adoptable",
            "not_archived",
            "declared_apply_open",
            "admission_unsettled",
            "predecessor_mismatch",
            "import_refused",
            "purge_refused",
            "preparation_not_pending",
            "preparation_mismatch",
            "construction_open",
            "stale_attempt",
            "not_constructed",
            "construction_required",
            "activation_pending",
            // Decisions.
            "approval_unavailable",
            "decision_open",
            "subject_widened",
            "decision_closed",
            "decision_not_found",
            // Decide side.
            "evidence_required",
            "subject_changed",
            "presentation_incomplete",
            "requester_cannot_decide",
            "approver_not_eligible",
            "decided_before_presentation",
            // Admission.
            "changes_held",
            "host_preparation_pending",
            "candidate_launch",
            "identity_member",
            "storage_unsupported",
            "mob_not_running",
            "purged",
            "construction_refused",
        ],
        "one vocabulary for every identity: no template, creator or limit kind"
    );
    let distinct: BTreeSet<&str> = tokens.iter().copied().collect();
    assert_eq!(distinct.len(), 57);
    let retryable: BTreeSet<&str> = kinds
        .iter()
        .filter(|kind| kind.is_retryable())
        .map(IdentityRefusalKind::token)
        .collect();
    assert_eq!(
        retryable,
        BTreeSet::from([
            "admission_unsettled",
            "changes_held",
            "consequences_unavailable",
            "declared_apply_open",
            "host_preparation_pending",
            "not_adoptable",
        ])
    );
    let problems: Vec<&str> = [
        RequestProblem::Empty,
        RequestProblem::Conflicting,
        RequestProblem::Malformed,
    ]
    .into_iter()
    .map(RequestProblem::token)
    .collect();
    assert_eq!(problems, ["empty", "conflicting", "malformed"]);
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
}

#[test]
fn refusal_data_carries_the_kind_its_field_and_its_payload() {
    let cases = [
        (
            IdentityRefusal::new(IdentityRefusalKind::ProfileNotAllowed {
                profile: "ghost".to_owned(),
            })
            .at("agents[0].profile"),
            json!({
                "kind": "profile_not_allowed",
                "profile": "ghost",
                "field": "agents[0].profile"
            }),
        ),
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
        (
            IdentityRefusal::new(IdentityRefusalKind::ConstructionRefused {
                kind: "controller_route_refused".to_owned(),
            }),
            json!({"kind": "construction_refused", "construction": "controller_route_refused"}),
        ),
        (
            IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            })
            .at("agents[0].labels"),
            json!({
                "kind": "invalid_request",
                "problem": "malformed",
                "field": "agents[0].labels"
            }),
        ),
    ];
    for (refusal, data) in cases {
        assert_eq!(refusal.to_data(), data, "{refusal}");
    }
}
