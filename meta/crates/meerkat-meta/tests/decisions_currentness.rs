#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Meta answers Core's approver currentness from its one live approver map:
//! a decision's human decider is current while the map still lets them act
//! for the decision's approver id, and refused once the map no longer does,
//! so an approval whose approver was removed between the decision and its
//! execution gets no execution permit. A runtime with no access controller
//! has no map, and refuses.
//!
//! The stdio connection's decide reads the same map when the decision
//! commits: the operator decides only while the map lists it, on the
//! connection that presented the decision, while that connection is open;
//! a candidate launch presents nothing and binds no attested event; and a
//! metadata store that cannot keep the attested events refuses the
//! attested decide.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use meerkat_client::TestClient;
use meerkat_core::approval::ApprovalAssurance;
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
    HostAuthenticationScope,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, DecisionHoldRequest, DecisionIngress, DecisionKind,
    DecisionOperationId, DecisionPhase, DecisionPrincipalKey, DecisionReference, DecisionRenderer,
    DecisionRendering, DecisionRequestBody, DecisionSubjectKey, PresentationAudience,
    PresentationRecord, Sha256Digest, StoredDecision,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_meta::{
    AccessControlConfig, AccessController, DecisionNoticeSink, InMemoryMetadataStore,
    PersistentMetadataStore, RuntimeDecisions, SqliteMetadataStore, StdioDecisionConnection,
    UnifiedRuntime,
};
use meerkat_mob::MobDefinition;
use meerkat_runtime::decision::{
    APPROVER_NOT_CURRENT_REFUSAL, DecisionApproverCurrentness, DecisionError, DecisionRefusal,
    DurableDecisionService,
};
use serde_json::{Value, json};

const RENDERING: &str = "Run the test kind's operation once.";

const MAP_WITH_HOLDER: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#;

const MAP_WITHOUT_HOLDER: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
"#;

/// The map that lets the stdio operator decide for `team:owners`.
const OPERATOR_MAP: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "operator" }]
"#;

/// The decider is attested by the test, as the host's own ingress would
/// attest it: eligible when the decision commits.
struct AttestedHere;

impl ConsentEligibility for AttestedHere {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
}

async fn runtime(state: &Path, map: Option<&str>) -> UnifiedRuntime {
    runtime_with(state, map, None).await
}

/// A persistent runtime on `state` that declares the `chat` ingress, with
/// the approver map `map` and, when given, the metadata store `metadata`.
async fn runtime_with(
    state: &Path,
    map: Option<&str>,
    metadata: Option<Arc<dyn PersistentMetadataStore>>,
) -> UnifiedRuntime {
    let mut builder = UnifiedRuntime::builder()
        .definition(
            MobDefinition::from_toml(
                r#"
[mob]
id = "decisions-currentness"

[profiles.lead]
model = "gpt-5.5"
"#,
            )
            .expect("definition"),
        )
        .persistent_state(state)
        .default_llm_client(Arc::new(TestClient::default()))
        .decision_ingresses(chat_declared());
    if let Some(map) = map {
        let config: AccessControlConfig = toml::from_str(map).expect("map");
        builder = builder.access_controller(AccessController::new(config).expect("controller"));
    }
    if let Some(metadata) = metadata {
        builder = builder.persistent_metadata(metadata);
    }
    Box::pin(builder.build()).await.expect("runtime")
}

fn chat_declared() -> meerkat_meta::DecisionIngressDeclarations {
    serde_json::from_value(json!({"ingresses": [
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
    ]}))
    .expect("ingress declarations")
}

/// The decision owner of the state directory `state`, as another holder on
/// the store opens it.
fn owner_of(state: &Path) -> DurableDecisionService {
    meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.to_path_buf(),
    }
    .open_decisions()
}

fn agent_requester() -> DecisionPrincipalKey {
    DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::PersonalAgent,
            "builder",
            TrustDomainId::new("example.org").expect("domain"),
        )
        .expect("principal"),
    )
    .expect("key")
}

/// Hold one decision for `team:owners`.
fn hold(owner: &DurableDecisionService, subject: &str) -> StoredDecision {
    owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("release").expect("kind"),
                DecisionSubjectKey::new(subject).expect("subject"),
                DecisionOperationId::new(format!("operation:{subject}")).expect("operation"),
                Sha256Digest::of(b"{}"),
                DecisionRequestBody::from_value(&json!({})).expect("body"),
                agent_requester(),
                ApproverId::new("team:owners").expect("approver"),
                u64::MAX / 2,
                DecisionRendering::new(RENDERING).expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold")
}

/// Record that a host channel showed `held` to `team:owners`, now.
fn present_to_owners(owner: &DurableDecisionService, held: &StoredDecision) -> u64 {
    let at_ms = now_ms();
    owner
        .present(
            &held.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new("team:owners").expect("approver"),
                },
                &DecisionRendering::new(RENDERING).expect("rendering"),
                None,
                DecisionReference::new("chat:owners").expect("channel"),
                DecisionReference::new("message-7").expect("message"),
                at_ms,
            ),
        )
        .expect("present");
    at_ms
}

/// Hold, present and approve one decision of a test kind, decided by the
/// chat person `U123` for `team:owners`.
fn approved(owner: &DurableDecisionService) -> StoredDecision {
    let held = owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("test.operation").expect("kind"),
                DecisionSubjectKey::new("operation-1").expect("subject"),
                DecisionOperationId::new("operation:1").expect("operation"),
                Sha256Digest::of(b"{}"),
                DecisionRequestBody::from_value(&json!({})).expect("body"),
                DecisionPrincipalKey::of(
                    &PrincipalRef::in_domain(
                        PrincipalKind::PersonalAgent,
                        "builder",
                        TrustDomainId::new("example.org").expect("domain"),
                    )
                    .expect("principal"),
                )
                .expect("key"),
                ApproverId::new("team:owners").expect("approver"),
                u64::MAX / 2,
                DecisionRendering::new(RENDERING).expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold");
    let rendering = DecisionRendering::new(RENDERING).expect("rendering");
    let at_ms = now_ms();
    owner
        .present(
            &held.decision_id,
            &PresentationRecord::complete(
                PresentationAudience::Approver {
                    approver_id: ApproverId::new("team:owners").expect("approver"),
                },
                &rendering,
                None,
                DecisionReference::new("chat:owners").expect("channel"),
                DecisionReference::new("message-1").expect("message"),
                at_ms,
            ),
        )
        .expect("present");
    let person = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "U123",
        TrustDomainId::new("meta-ingress-chat").expect("domain"),
    )
    .expect("person");
    let approver = AuthenticatedApprover::new(person, ApprovalAssurance::PersonAuthenticated)
        .expect("approver")
        .with_authentication_scope(HostAuthenticationScope::new("chat-session-1"));
    let decision = AttestedDecision::new(
        approver,
        DecisionIngress::new("chat").expect("ingress"),
        DecisionReference::new("chat:event-1").expect("event"),
        at_ms,
        ApproverId::new("team:owners").expect("approver"),
        rendering.subject_digest(),
        true,
    )
    .expect("decision");
    let decided = owner
        .decide(&held.decision_id, &decision, &AttestedHere)
        .expect("decide");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
    decided
}

/// The notices one stdio connection wrote, each acknowledged as on the
/// pipe.
#[derive(Default)]
struct Notices(Mutex<Vec<Value>>);

#[async_trait]
impl DecisionNoticeSink for Notices {
    async fn deliver(&self, line: String) -> bool {
        let line: Value = serde_json::from_str(&line).expect("a notice line is JSON");
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line["params"].clone());
        true
    }
}

impl Notices {
    /// The events of the notices written for `decision`, in order.
    fn events_of(&self, decision: &StoredDecision) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|notice| notice["decision_id"] == json!(decision.decision_id.to_string()))
            .map(|notice| notice["event"].as_str().expect("event").to_string())
            .collect()
    }
}

/// A stdio connection of `runtime`, as the gateway opens one per process,
/// and the notices it writes.
fn open_connection(runtime: &UnifiedRuntime) -> (StdioDecisionConnection, Arc<Notices>) {
    let notices = Arc::new(Notices::default());
    let sink: Arc<dyn DecisionNoticeSink> = notices.clone();
    (
        StdioDecisionConnection::new(runtime.durable_decisions().realm(), sink),
        notices,
    )
}

/// One `meta/decisions/*` call on `connection`.
async fn call(
    connection: &StdioDecisionConnection,
    runtime: &UnifiedRuntime,
    method: &str,
    params: Value,
) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": "decisions", "method": method, "params": params})
        .to_string();
    let answer = connection
        .handle_line(runtime, &line)
        .await
        .expect("a decision method answers");
    serde_json::from_str(&answer).expect("answer JSON")
}

/// The stdio operator's approval of `decision`.
fn operator_approves(decision: &StoredDecision) -> Value {
    json!({
        "decision_id": decision.decision_id.to_string(),
        "approve": true,
        "presented_digest": decision.descriptor.subject_digest.to_string(),
    })
}

/// The chat person `subject`'s approval of `decision`, observed now.
fn chat_approves(decision: &StoredDecision, subject: &str, event_ref: &str) -> Value {
    json!({
        "decision_id": decision.decision_id.to_string(),
        "approve": true,
        "presented_digest": decision.descriptor.subject_digest.to_string(),
        "attestation": {
            "ingress": "chat",
            "subject": subject,
            "scope": "chat-session-1",
            "event_ref": event_ref,
            "observed_at_ms": now_ms(),
        },
    })
}

fn assert_refused(answer: &Value, refusal: &str) {
    assert_eq!(answer["error"]["code"], json!(-32602), "{answer}");
    assert_eq!(
        answer["error"]["data"]["kind"],
        json!("decision_refused"),
        "{answer}"
    );
    assert_eq!(
        answer["error"]["data"]["decision_refusal"]["kind"],
        json!(refusal),
        "{answer}"
    );
}

fn replace_map(runtime: &UnifiedRuntime, map: &str) {
    runtime
        .access_controller()
        .expect("controller")
        .replace_config(toml::from_str(map).expect("map"))
        .expect("replace the map");
}

fn assert_still_open(owner: &DurableDecisionService, decision: &StoredDecision) {
    let row = owner.get(&decision.decision_id).expect("row");
    assert_eq!(row.state.phase, DecisionPhase::Open);
    assert!(row.outcome.is_none());
}

#[tokio::test]
async fn approver_removed_from_live_map_between_decide_and_execute_gets_no_permit() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(MAP_WITH_HOLDER)).await;
    let owner = meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.clone(),
    }
    .open_decisions();
    let decided = approved(&owner);
    let currentness: Arc<dyn DecisionApproverCurrentness> = runtime.decision_approver_currentness();
    assert!(
        matches!(
            currentness.approver_answer(&decided),
            ConsentEligibilityAnswer::Eligible(_)
        ),
        "the map lists the decider"
    );

    runtime
        .access_controller()
        .expect("controller")
        .replace_config(toml::from_str(MAP_WITHOUT_HOLDER).expect("map"))
        .expect("replace the map");
    let answer = currentness.approver_answer(&decided);
    assert!(matches!(answer, ConsentEligibilityAnswer::Refused));
    let refused = owner.authorize_execution(
        &decided.decision_id,
        &decided.descriptor.operation_id,
        &decided.descriptor.request_digest,
        &answer,
    );
    assert!(
        matches!(
            refused,
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotEligible,
                ..
            })
        ),
        "{refused:?}"
    );
    let ended = owner.get(&decided.decision_id).expect("row");
    assert_eq!(ended.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!ended.state.permit_issued);
    runtime.shutdown().await;
}

#[tokio::test]
async fn currentness_without_an_approver_map_refuses() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, None).await;
    let owner = meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.clone(),
    }
    .open_decisions();
    let decided = approved(&owner);
    assert!(matches!(
        runtime
            .decision_approver_currentness()
            .approver_answer(&decided),
        ConsentEligibilityAnswer::Refused
    ));
    runtime.shutdown().await;
}

#[tokio::test]
async fn stale_currentness_evidence_is_unsettled_after_a_map_change() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(MAP_WITH_HOLDER)).await;
    let owner = owner_of(&state);
    let decided = approved(&owner);
    let currentness = runtime.decision_approver_currentness();
    let stale = currentness.approver_answer(&decided);
    assert!(
        matches!(stale, ConsentEligibilityAnswer::Eligible(_)),
        "the map lists the decider when the answer is read"
    );

    replace_map(&runtime, MAP_WITHOUT_HOLDER);
    let unsettled = owner.authorize_execution(
        &decided.decision_id,
        &decided.descriptor.operation_id,
        &decided.descriptor.request_digest,
        &stale,
    );
    assert!(
        matches!(unsettled, Err(DecisionError::Unsettled(_))),
        "an answer read before the edit is unsettled at the commit: {unsettled:?}"
    );
    let untouched = owner.get(&decided.decision_id).expect("row");
    assert_eq!(untouched.state.phase, DecisionPhase::Approved);
    assert!(!untouched.state.permit_issued, "nothing was applied");

    let fresh = currentness.approver_answer(&decided);
    assert!(matches!(fresh, ConsentEligibilityAnswer::Refused));
    let refused = owner.authorize_execution(
        &decided.decision_id,
        &decided.descriptor.operation_id,
        &decided.descriptor.request_digest,
        &fresh,
    );
    assert!(
        matches!(
            refused,
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotEligible,
                ..
            })
        ),
        "{refused:?}"
    );
    let ended = owner.get(&decided.decision_id).expect("row");
    assert_eq!(ended.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!ended.state.permit_issued);
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_operator_removed_from_the_map_after_its_presentation_gets_not_eligible() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(OPERATOR_MAP)).await;
    let owner = owner_of(&state);
    let held = hold(&owner, "operation-7");
    let (connection, notices) = open_connection(&runtime);

    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(notices.events_of(&held), ["presented"]);

    replace_map(&runtime, MAP_WITHOUT_HOLDER);
    let decided = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        operator_approves(&held),
    )
    .await;
    assert_refused(&decided, "not_eligible");
    assert_still_open(&owner, &held);
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_operator_decided_decision_loses_its_permit_when_the_operator_leaves_the_map() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(OPERATOR_MAP)).await;
    let owner = owner_of(&state);
    let held = hold(&owner, "operation-7");
    let (connection, _notices) = open_connection(&runtime);
    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    let decided = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        operator_approves(&held),
    )
    .await;
    assert!(decided["error"].is_null(), "{decided}");
    assert_eq!(
        decided["result"]["decision"]["outcome"]["evidence"]["ingress"],
        json!("operator")
    );
    let decided = owner.get(&held.decision_id).expect("row");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);

    let currentness = runtime.decision_approver_currentness();
    assert!(
        matches!(
            currentness.approver_answer(&decided),
            ConsentEligibilityAnswer::Eligible(_)
        ),
        "the map lists the operator that decided"
    );
    replace_map(&runtime, MAP_WITHOUT_HOLDER);
    let answer = currentness.approver_answer(&decided);
    assert!(matches!(answer, ConsentEligibilityAnswer::Refused));
    let refused = owner.authorize_execution(
        &decided.decision_id,
        &decided.descriptor.operation_id,
        &decided.descriptor.request_digest,
        &answer,
    );
    assert!(
        matches!(
            refused,
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotEligible,
                ..
            })
        ),
        "{refused:?}"
    );
    let ended = owner.get(&decided.decision_id).expect("row");
    assert_eq!(ended.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!ended.state.permit_issued);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_decide_on_a_closed_connection_is_not_eligible() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(OPERATOR_MAP)).await;
    let owner = owner_of(&state);
    let held = hold(&owner, "operation-7");
    let (connection, notices) = open_connection(&runtime);
    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(notices.events_of(&held), ["presented"]);

    connection.close();
    let decided = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        operator_approves(&held),
    )
    .await;
    assert_refused(&decided, "not_eligible");
    assert_still_open(&owner, &held);

    // An open connection that presents the decision decides it.
    let (reopened, _notices) = open_connection(&runtime);
    let listed = call(&reopened, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    let decided = call(
        &reopened,
        &runtime,
        "meta/decisions/decide",
        operator_approves(&held),
    )
    .await;
    assert!(decided["error"].is_null(), "{decided}");
    assert_eq!(decided["result"]["decision"]["phase"], json!("approved"));
    runtime.shutdown().await;
}

/// The decisions of a candidate launch on `state`: they read the store and
/// refuse every write, and record attested events in `events` under
/// `mob_id`.
fn candidate_decisions(
    runtime: &UnifiedRuntime,
    state: &Path,
    events: Arc<dyn PersistentMetadataStore>,
    mob_id: &str,
) -> RuntimeDecisions {
    let realm = runtime
        .durable_decisions()
        .realm()
        .cloned()
        .expect("a persistent runtime's realm");
    RuntimeDecisions::in_state_dir(state, realm, chat_declared(), events, mob_id, false)
}

#[tokio::test]
async fn a_candidate_launch_attested_decide_binds_no_event() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let mut runtime = runtime(&state, Some(MAP_WITH_HOLDER)).await;
    let events: Arc<dyn PersistentMetadataStore> = Arc::new(
        SqliteMetadataStore::open(dir.path().join("events.sqlite3")).expect("event store"),
    );
    let candidate = candidate_decisions(&runtime, &state, Arc::clone(&events), "candidate-mob");
    runtime.set_durable_decisions(candidate);
    let owner = owner_of(&state);
    let held = hold(&owner, "operation-7");
    present_to_owners(&owner, &held);
    let (connection, _notices) = open_connection(&runtime);

    let refused = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        chat_approves(&held, "U123", "chat:event-1"),
    )
    .await;
    assert_eq!(
        refused["error"]["data"]["kind"],
        json!("candidate_launch"),
        "{refused}"
    );
    assert_eq!(
        events
            .bind_decision_event("candidate-mob", "chat", "chat:event-1", "decision:another")
            .await
            .expect("bind"),
        "decision:another",
        "the refused decide bound no event"
    );
    assert_still_open(&owner, &held);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_candidate_launch_presents_nothing_to_the_operator() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let mut runtime = runtime(&state, Some(OPERATOR_MAP)).await;
    let events: Arc<dyn PersistentMetadataStore> = Arc::new(
        SqliteMetadataStore::open(dir.path().join("events.sqlite3")).expect("event store"),
    );
    let candidate = candidate_decisions(&runtime, &state, events, "candidate-mob");
    runtime.set_durable_decisions(candidate);
    let owner = owner_of(&state);
    let held = hold(&owner, "operation-7");
    let (connection, notices) = open_connection(&runtime);

    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    assert_eq!(
        notices.events_of(&held),
        ["held"],
        "a candidate launch notices the decision and shows it to no one"
    );
    let row = owner.get(&held.decision_id).expect("row");
    assert!(row.presentations.is_empty(), "nothing was presented");
    assert_eq!(row.state.phase, DecisionPhase::Open);
    runtime.shutdown().await;
}

#[tokio::test]
async fn operator_ingress_evidence_for_another_decider_is_not_vouched() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(OPERATOR_MAP)).await;
    let owner = owner_of(&state);

    // A host adapter on the shared store records the ingress `operator` for
    // a person who is not this host's operator.
    let held = hold(&owner, "operation-7");
    let at_ms = present_to_owners(&owner, &held);
    let someone = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "someone-else",
        TrustDomainId::new("example.org").expect("domain"),
    )
    .expect("principal");
    let approver = AuthenticatedApprover::new(someone, ApprovalAssurance::DeviceTrusted)
        .expect("approver")
        .with_authentication_scope(HostAuthenticationScope::new("another-host"));
    let decision = AttestedDecision::new(
        approver,
        DecisionIngress::new("operator").expect("ingress"),
        DecisionReference::new("another-host:event-1").expect("event"),
        at_ms,
        ApproverId::new("team:owners").expect("approver"),
        held.descriptor.subject_digest.clone(),
        true,
    )
    .expect("decision");
    let decided = owner
        .decide(&held.decision_id, &decision, &AttestedHere)
        .expect("decide");
    let currentness = runtime.decision_approver_currentness();
    assert!(
        matches!(
            currentness.approver_answer(&decided),
            ConsentEligibilityAnswer::Refused
        ),
        "Meta vouches for operator evidence only when its decider is this host's operator"
    );

    // This host's operator, deciding through its own connection, is vouched
    // for.
    let own = hold(&owner, "operation-8");
    let (connection, _notices) = open_connection(&runtime);
    let listed = call(&connection, &runtime, "meta/decisions/list", json!({})).await;
    assert!(listed["error"].is_null(), "{listed}");
    let answer = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        operator_approves(&own),
    )
    .await;
    assert!(answer["error"].is_null(), "{answer}");
    let own = owner.get(&own.decision_id).expect("row");
    assert!(matches!(
        currentness.approver_answer(&own),
        ConsentEligibilityAnswer::Eligible(_)
    ));
    runtime.shutdown().await;
}

#[tokio::test]
async fn an_attested_decide_over_a_store_that_cannot_keep_events_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    // Persistent decisions, but a metadata store that forgets its records at
    // a restart: the attested event cannot be kept to one decision.
    let forgetful: Arc<dyn PersistentMetadataStore> = Arc::new(InMemoryMetadataStore::new());
    let runtime = runtime_with(&state, Some(MAP_WITH_HOLDER), Some(forgetful)).await;
    let owner = owner_of(&state);
    let held = hold(&owner, "operation-7");
    present_to_owners(&owner, &held);
    let (connection, _notices) = open_connection(&runtime);

    let refused = call(
        &connection,
        &runtime,
        "meta/decisions/decide",
        chat_approves(&held, "U123", "chat:event-1"),
    )
    .await;
    assert!(
        refused["error"].is_object() && refused["result"].is_null(),
        "the attested decide is refused: {refused}"
    );
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("decision event")),
        "the refusal names the event record: {refused}"
    );
    assert_still_open(&owner, &held);
    runtime.shutdown().await;
}

#[tokio::test]
async fn a_link_added_after_the_decision_ends_it_execution_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let state = dir.path().join("state");
    let runtime = runtime(&state, Some(MAP_WITH_HOLDER)).await;
    let owner = owner_of(&state);
    let requester = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "U999",
        TrustDomainId::new("meta-ingress-chat").expect("domain"),
    )
    .expect("requester");
    let held = owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("test.operation").expect("kind"),
                DecisionSubjectKey::new("probe-1").expect("subject"),
                DecisionOperationId::new("operation:probe-1").expect("operation"),
                Sha256Digest::of(b"{}"),
                DecisionRequestBody::from_value(&json!({})).expect("body"),
                DecisionPrincipalKey::of(&requester).expect("key"),
                ApproverId::new("team:owners").expect("approver"),
                u64::MAX / 2,
                DecisionRendering::new(RENDERING).expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold");
    let at_ms = present_to_owners(&owner, &held);
    let person = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "U123",
        TrustDomainId::new("meta-ingress-chat").expect("domain"),
    )
    .expect("person");
    let approver = AuthenticatedApprover::new(person, ApprovalAssurance::PersonAuthenticated)
        .expect("approver")
        .with_authentication_scope(HostAuthenticationScope::new("chat-session-1"));
    let decision = AttestedDecision::new(
        approver,
        DecisionIngress::new("chat").expect("ingress"),
        DecisionReference::new("chat:probe-event-1").expect("event"),
        at_ms,
        ApproverId::new("team:owners").expect("approver"),
        held.descriptor.subject_digest.clone(),
        true,
    )
    .expect("decision");
    let decided = owner
        .decide(&held.decision_id, &decision, &AttestedHere)
        .expect("decide");
    assert_eq!(decided.state.phase, DecisionPhase::Approved);
    let currentness = runtime.decision_approver_currentness();
    assert!(matches!(
        currentness.approver_answer(&decided),
        ConsentEligibilityAnswer::Eligible(_)
    ));

    // The map now says the decider U123 and the requester U999 are one
    // person.
    replace_map(
        &runtime,
        r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "chat", subject = "U999" }] },
]
"#,
    );
    let answer = currentness.approver_answer(&decided);
    assert!(
        matches!(answer, ConsentEligibilityAnswer::Refused),
        "the answer from the linked snapshot refuses the decider"
    );
    let authorized = owner.authorize_execution(
        &decided.decision_id,
        &decided.descriptor.operation_id,
        &decided.descriptor.request_digest,
        &answer,
    );
    assert!(
        matches!(
            authorized,
            Err(DecisionError::Refused {
                refusal: DecisionRefusal::NotEligible,
                ..
            })
        ),
        "not a permit, not a deferral: {authorized:?}"
    );
    let row = owner.get(&decided.decision_id).expect("row");
    assert_eq!(row.state.phase, DecisionPhase::ExecutionRefused);
    assert!(!row.state.permit_issued);
    assert_eq!(
        row.execution_refusal,
        Some(DecisionReference::new(APPROVER_NOT_CURRENT_REFUSAL).expect("reference"))
    );
    runtime.shutdown().await;
}
