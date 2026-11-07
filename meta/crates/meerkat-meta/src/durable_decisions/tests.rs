//! Unit rows of Meta's durable decision surfaces: the ingress
//! declarations, the approver map's holders and people, and the record of
//! attested decide events.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use meerkat_core::approval::consent::ConsentEligibility;
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPrincipalKey,
    DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
    StoredDecision,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use serde_json::json;

use super::approvers::{
    DeciderIdentity, LiveHolderEligibility, decider_principal, holding, ingress_person,
};
use super::ingress::{
    DecisionIngressDeclaration, DecisionIngressDeclarations, check_approver_ingresses,
};
use super::wire::{MetaDecisionNotice, MetaDecisionNoticeEvent};
use crate::access::{AccessConfigError, AccessControlConfig, AccessController};
use crate::runtime::metadata::{
    InMemoryMetadataStore, PersistentMetadataStore, SqliteMetadataStore,
};

fn declarations(value: serde_json::Value) -> Result<DecisionIngressDeclarations, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

fn chat_and_mail() -> DecisionIngressDeclarations {
    declarations(json!({"ingresses": [
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
        {"ingress": "mail", "attests": "person", "max_clock_skew_ms": 0},
    ]}))
    .expect("declarations")
}

fn map(toml: &str) -> AccessControlConfig {
    toml::from_str(toml).expect("access TOML")
}

#[test]
fn declarations_keep_each_ingress_and_its_skew() {
    let declared = chat_and_mail();
    assert_eq!(
        declared
            .get("chat")
            .map(DecisionIngressDeclaration::max_clock_skew_ms),
        Some(5000)
    );
    assert_eq!(
        declared
            .get("mail")
            .map(DecisionIngressDeclaration::max_clock_skew_ms),
        Some(0)
    );
    assert!(declared.get("operator").is_none());
    assert!(declared.get("sms").is_none());
    let names: Vec<&str> = declared
        .iter()
        .map(DecisionIngressDeclaration::ingress)
        .collect();
    assert_eq!(names, ["chat", "mail"]);
    let round_trip: DecisionIngressDeclarations =
        serde_json::from_value(serde_json::to_value(&declared).expect("serialize")).expect("parse");
    assert_eq!(round_trip, declared);
    assert!(declarations(json!({})).expect("empty").is_empty());
}

#[test]
fn declarations_refuse_reserved_invalid_duplicate_and_unsupported_ingresses() {
    let refused = |declaration: serde_json::Value| {
        declarations(json!({"ingresses": [declaration]})).expect_err("refused")
    };
    assert!(
        refused(json!({"ingress": "operator", "attests": "person", "max_clock_skew_ms": 1}))
            .contains("operator")
    );
    assert!(
        refused(json!({"ingress": "Chat!", "attests": "person", "max_clock_skew_ms": 1}))
            .contains("Chat!")
    );
    assert!(
        refused(json!({"ingress": "", "attests": "person", "max_clock_skew_ms": 1}))
            .contains("not an ingress name")
    );
    assert!(
        refused(json!({"ingress": "x".repeat(65), "attests": "person", "max_clock_skew_ms": 1}))
            .contains("not an ingress name")
    );
    assert!(
        refused(json!({"ingress": "chat", "attests": "service", "max_clock_skew_ms": 1}))
            .contains("attests")
    );
    assert!(refused(json!({"ingress": "chat", "attests": "person"})).contains("max_clock_skew_ms"));
    assert!(
        refused(
            json!({"ingress": "chat", "attests": "person", "max_clock_skew_ms": 1, "trust": 1})
        )
        .contains("trust")
    );
    let twice = declarations(json!({"ingresses": [
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 1},
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 2},
    ]}))
    .expect_err("refused");
    assert!(twice.contains("declared twice"), "{twice}");
}

#[test]
fn the_approver_map_names_only_declared_ingresses_and_the_operator() {
    let listed = map(r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "operator" }, { ingress = "chat", subject = "U1" }]
people = [{ ingress_subjects = [{ ingress = "mail", subject = "owner" }] }]
"#);
    let controller = AccessController::new(listed.clone()).expect("controller");
    assert_eq!(
        check_approver_ingresses(Some(&controller), &chat_and_mail()),
        Ok(())
    );
    let undeclared = AccessController::new(listed).expect("controller");
    assert_eq!(
        check_approver_ingresses(Some(&undeclared), &DecisionIngressDeclarations::default()),
        Err(AccessConfigError::UnknownIngress {
            approver: "team:owners".to_string(),
            ingress: "chat".to_string(),
        })
    );
    let in_a_person = AccessController::new(map(r#"
[approvers."team:owners"]
people = [{ ingress_subjects = [{ ingress = "sms", subject = "+1" }] }]
"#))
    .expect("controller");
    let refused = check_approver_ingresses(Some(&in_a_person), &chat_and_mail())
        .expect_err("unknown ingress");
    assert!(
        refused.to_string().contains("unknown ingress \"sms\""),
        "{refused}"
    );
    assert_eq!(
        check_approver_ingresses(None, &DecisionIngressDeclarations::default()),
        Ok(())
    );
}

#[test]
fn a_live_edit_may_name_only_the_declared_ingresses() {
    let controller = AccessController::new(map(r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "chat", subject = "U1" }]
"#))
    .expect("controller");
    // Before the launch declares its ingresses, nothing binds an edit.
    let undeclared_edit = map(r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "sms", subject = "+1" }]
"#);
    let before = controller.snapshot().1;
    check_approver_ingresses(Some(&controller), &chat_and_mail()).expect("declared");
    let refused = controller
        .replace_config(undeclared_edit)
        .expect_err("an undeclared ingress is refused");
    assert_eq!(refused.kind(), "unknown_ingress");
    assert!(refused.to_string().contains("\"sms\""), "{refused}");
    assert_eq!(controller.snapshot().1, before, "nothing was committed");
    controller
        .replace_config(map(r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "mail", subject = "owner" }, { ingress = "operator" }]
"#))
        .expect("a declared ingress and the operator are accepted");
}

#[test]
fn the_access_model_refuses_malformed_ingress_identities() {
    let refused = |toml: &str| {
        crate::access::validate_access_config(&map(toml))
            .expect_err("refused")
            .to_string()
    };
    assert!(
        refused(
            r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "operator", subject = "uid-1" }]
"#
        )
        .contains("operator")
    );
    for subject in ["", " U1", "U1 "] {
        let message = refused(&format!(
            r#"
[approvers."team:owners"]
ingress_subjects = [{{ ingress = "chat", subject = "{subject}" }}]
"#
        ));
        assert!(message.contains("team:owners"), "{message}");
    }
    assert!(
        refused(
            r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "chat" }]
"#
        )
        .contains("needs a subject")
    );
    assert!(
        refused(
            r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "Chat", subject = "U1" }]
"#
        )
        .contains("not an ingress name")
    );
    assert!(
        refused(
            r#"
[approvers."team:owners"]
people = [{}]
"#
        )
        .contains("names no identity")
    );
    // An approver held only through ingress identities or people has
    // holders.
    assert_eq!(
        crate::access::validate_access_config(&map(r#"
[approvers."team:owners"]
people = [{ ingress_subjects = [{ ingress = "chat", subject = "U1" }] }]
"#)),
        Ok(())
    );
}

#[test]
fn holders_come_from_ingress_subjects_and_people() {
    let config = map(r#"
[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "operator" }, { ingress = "chat", subject = "U555" }]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "mail", subject = "owner" }] },
]

[approvers."team:other"]
subjects = ["other@example.org"]
"#);
    let chat = |subject: &str| DeciderIdentity::Ingress {
        ingress: "chat".to_string(),
        subject: subject.to_string(),
    };
    assert!(holding(&config, "team:owners", &DeciderIdentity::Operator).is_some());
    assert!(holding(&config, "team:owners", &chat("U555")).is_some());
    assert!(holding(&config, "team:owners", &chat("U123")).is_some());
    assert!(holding(&config, "team:owners", &chat("U999")).is_none());
    assert!(holding(&config, "team:other", &DeciderIdentity::Operator).is_none());
    assert!(holding(&config, "team:absent", &chat("U555")).is_none());
    // The same subject through another ingress is another identity.
    assert!(
        holding(
            &config,
            "team:owners",
            &DeciderIdentity::Ingress {
                ingress: "mail".to_string(),
                subject: "U555".to_string(),
            }
        )
        .is_none()
    );
}

fn requested_by(requester: &PrincipalRef) -> StoredDecision {
    let state = tempfile::tempdir().expect("state dir");
    let owner = meerkat::ApprovalIssuerSelection::Persistent {
        store_path: state.path().to_path_buf(),
    }
    .open_decisions();
    owner
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("release").expect("kind"),
                DecisionSubjectKey::new("build-7").expect("subject"),
                DecisionOperationId::new("operation:build-7").expect("operation"),
                Sha256Digest::of(b"{}"),
                DecisionRequestBody::from_value(&json!({})).expect("body"),
                DecisionPrincipalKey::of(requester).expect("key"),
                ApproverId::new("team:owners").expect("approver"),
                u64::MAX / 2,
                DecisionRendering::new("Release build 7.").expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold")
}

#[test]
fn a_person_linked_to_the_requester_decides_as_the_requester() {
    let config = map(r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "chat", subject = "U555" }]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "mail", subject = "owner" }, { ingress = "operator" }] },
]
"#);
    let requester = ingress_person("mail", "owner").expect("requester");
    let row = requested_by(&requester);
    let linked = DeciderIdentity::Ingress {
        ingress: "chat".to_string(),
        subject: "U123".to_string(),
    };
    let own = ingress_person("chat", "U123").expect("decider");
    let held = holding(&config, "team:owners", &linked);
    assert_eq!(
        decider_principal(&row, held.as_ref(), own.clone(), None),
        requester
    );
    // A holder linked to nobody decides as itself.
    let other = DeciderIdentity::Ingress {
        ingress: "chat".to_string(),
        subject: "U555".to_string(),
    };
    let own_other = ingress_person("chat", "U555").expect("decider");
    let held_other = holding(&config, "team:owners", &other);
    assert_eq!(
        decider_principal(&row, held_other.as_ref(), own_other.clone(), None),
        own_other
    );
    // The operator in the same person is the requester's own too.
    let operator = PrincipalRef::in_domain(
        PrincipalKind::Human,
        "uid-1000",
        TrustDomainId::new("rkat-realm-meta").expect("domain"),
    )
    .expect("operator");
    let held_operator = holding(&config, "team:owners", &DeciderIdentity::Operator);
    assert_eq!(
        decider_principal(
            &row,
            held_operator.as_ref(),
            operator.clone(),
            Some(&operator)
        ),
        requester
    );
    // Without a holding there is nothing to link.
    assert_eq!(decider_principal(&row, None, own.clone(), None), own);
}

#[test]
fn an_ingress_person_is_a_human_in_the_ingress_domain() {
    let person = ingress_person("chat", "U123").expect("person");
    assert_eq!(person.kind, PrincipalKind::Human);
    assert_eq!(person.id.as_str(), "U123");
    assert_eq!(
        DecisionPrincipalKey::of(&person).expect("key").to_string(),
        r#"principal:["human","meta-ingress-chat","U123"]"#
    );
}

#[tokio::test]
async fn the_first_decision_keeps_an_attested_event() {
    let dir = tempfile::tempdir().expect("dir");
    let store: Arc<dyn PersistentMetadataStore> = Arc::new(
        SqliteMetadataStore::open(dir.path().join("metadata.sqlite3")).expect("sqlite store"),
    );
    let bind = |decision: &'static str, event: &'static str| {
        let store = Arc::clone(&store);
        async move {
            store
                .bind_decision_event("mob", "chat", event, decision)
                .await
                .expect("bind")
        }
    };
    assert_eq!(bind("decision:a", "chat:event-1").await, "decision:a");
    assert_eq!(bind("decision:a", "chat:event-1").await, "decision:a");
    assert_eq!(bind("decision:b", "chat:event-1").await, "decision:a");
    assert_eq!(bind("decision:b", "chat:event-2").await, "decision:b");
    // Another ingress's event of the same reference is another event.
    assert_eq!(
        store
            .bind_decision_event("mob", "mail", "chat:event-1", "decision:b")
            .await
            .expect("bind"),
        "decision:b"
    );
}

#[tokio::test]
async fn an_in_memory_metadata_store_keeps_no_decision_events() {
    // A record that a restart forgets cannot keep an event to one decision:
    // the in-memory store refuses to bind, and the attested decide with it.
    let store = InMemoryMetadataStore::new();
    let refused = store
        .bind_decision_event("mob", "chat", "chat:event-1", "decision:a")
        .await;
    assert!(
        refused.is_err(),
        "the in-memory store binds no decision event: {refused:?}"
    );
}

#[tokio::test]
async fn a_restarted_store_keeps_the_events_it_bound() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("metadata.sqlite3");
    {
        let store = SqliteMetadataStore::open(&path).expect("store");
        assert_eq!(
            store
                .bind_decision_event("mob", "chat", "chat:event-1", "decision:a")
                .await
                .expect("bind"),
            "decision:a"
        );
    }
    let reopened = SqliteMetadataStore::open(&path).expect("reopened");
    assert_eq!(
        reopened
            .bind_decision_event("mob", "chat", "chat:event-1", "decision:b")
            .await
            .expect("bind"),
        "decision:a"
    );
}

fn chat(subject: &str) -> DeciderIdentity {
    DeciderIdentity::Ingress {
        ingress: "chat".to_string(),
        subject: subject.to_string(),
    }
}

/// The stdio operator's principal, as a connection names it in its realm.
fn operator_principal() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        "uid-1000",
        TrustDomainId::new("rkat-realm-meta").expect("domain"),
    )
    .expect("operator")
}

/// The eligibility a stdio operator's decide on `team:owners` builds for
/// `row`, presented on an open connection, whose decide input carries
/// `operator`.
fn operator_eligibility(
    controller: &AccessController,
    row: &StoredDecision,
    operator: &PrincipalRef,
) -> LiveHolderEligibility {
    // This version's eligibility reads the holder only.
    let _ = (row, operator);
    LiveHolderEligibility::operator(
        Some(controller.clone()),
        "team:owners".to_string(),
        true,
        Arc::new(AtomicBool::new(true)),
    )
}

/// The eligibility an attested decide on `team:owners` builds for
/// `identity` and `row`, whose decide input carries `decides_as`.
fn attested_eligibility(
    controller: &AccessController,
    identity: DeciderIdentity,
    row: &StoredDecision,
    decides_as: &PrincipalRef,
) -> LiveHolderEligibility {
    // This version's eligibility reads the holder only.
    let _ = (row, decides_as);
    LiveHolderEligibility::attested(
        Some(controller.clone()),
        "team:owners".to_string(),
        identity,
    )
}

#[test]
fn live_holder_eligibility_reads_the_map_at_commit() {
    let listed = r#"
[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [{ ingress = "operator" }, { ingress = "chat", subject = "U123" }]
"#;
    let controller = AccessController::new(map(listed)).expect("controller");
    let row = requested_by(&ingress_person("mail", "someone").expect("requester"));
    let operator = operator_principal();
    let person = ingress_person("chat", "U123").expect("person");
    let operator_decides = operator_eligibility(&controller, &row, &operator);
    let person_decides = attested_eligibility(&controller, chat("U123"), &row, &person);
    assert!(
        operator_decides.check_current().is_ok(),
        "the map lists the operator"
    );
    assert!(
        person_decides.check_current().is_ok(),
        "the map lists the person"
    );

    // Built while the map listed them; the commit reads the map it finds.
    controller
        .replace_config(map(r#"
[approvers."team:owners"]
subjects = ["owner@example.org"]
"#))
        .expect("the edit lists neither");
    assert!(
        operator_decides.check_current().is_err(),
        "the operator's eligibility reads the map again at commit"
    );
    assert!(
        person_decides.check_current().is_err(),
        "the person's eligibility reads the map again at commit"
    );

    controller
        .replace_config(map(listed))
        .expect("listed again");
    assert!(operator_decides.check_current().is_ok());
    assert!(person_decides.check_current().is_ok());
}

#[test]
fn a_link_added_between_the_read_and_the_commit_is_not_eligible() {
    // The decide reads a map that links the chat person U123 to nobody: its
    // input carries their own key.
    let controller = AccessController::new(map(r#"
[approvers."team:owners"]
ingress_subjects = [{ ingress = "chat", subject = "U123" }]
"#))
    .expect("controller");
    let requester = ingress_person("mail", "owner").expect("requester");
    let row = requested_by(&requester);
    let own = ingress_person("chat", "U123").expect("decider");
    let read = holding(&controller.snapshot().0, "team:owners", &chat("U123"));
    let decides_as = decider_principal(&row, read.as_ref(), own.clone(), None);
    assert_eq!(decides_as, own, "the read links the decider to nobody");
    let eligibility = attested_eligibility(&controller, chat("U123"), &row, &decides_as);
    assert!(eligibility.check_current().is_ok());

    // Before the commit, an edit makes the decider and the requester one
    // person.
    controller
        .replace_config(map(r#"
[approvers."team:owners"]
people = [
  { ingress_subjects = [{ ingress = "chat", subject = "U123" }, { ingress = "mail", subject = "owner" }] },
]
"#))
        .expect("the edit links them");
    assert!(
        eligibility.check_current().is_err(),
        "a decide that carries the decider's own key is not eligible once the map links the \
         decider to the requester"
    );

    // The ordinary path: a decide that reads the linked map carries the
    // requester's key, stays eligible, and the machine refuses it as self
    // approval.
    let read_now = holding(&controller.snapshot().0, "team:owners", &chat("U123"));
    let decides_now = decider_principal(&row, read_now.as_ref(), own, None);
    assert_eq!(decides_now, requester);
    let read_after = attested_eligibility(&controller, chat("U123"), &row, &decides_now);
    assert!(read_after.check_current().is_ok());
}

#[test]
fn an_unknown_notice_event_decodes_as_unknown() {
    // Later notices name events this version does not know: a host decodes
    // them, as Core's notice vocabulary is decoded.
    let event: MetaDecisionNoticeEvent = serde_json::from_value(json!("decided_elsewhere"))
        .expect("a notice event this version does not know decodes");
    assert_eq!(format!("{event:?}"), "Unknown");
    let notice: MetaDecisionNotice = serde_json::from_value(json!({
        "notice_id": "notice-1",
        "decision_id": format!("decision:{}", "a".repeat(64)),
        "kind": "release",
        "event": "decided_elsewhere",
        "phase": "open",
        "approver_id": "team:owners",
        "requester": "principal:[\"personal_agent\",\"example.org\",\"builder\"]",
        "rendering_ref": Sha256Digest::of(b"Release build 7.").to_string(),
    }))
    .expect("a notice with a later event decodes");
    assert_eq!(format!("{:?}", notice.event), "Unknown");
    for known in ["held", "presented"] {
        let event: MetaDecisionNoticeEvent =
            serde_json::from_value(json!(known)).expect("a known event");
        assert_eq!(serde_json::to_value(event).expect("encode"), json!(known));
    }
}
