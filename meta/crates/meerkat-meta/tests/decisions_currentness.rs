#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Meta answers Core's approver currentness from its one live approver map:
//! a decision's human decider is current while the map still lets them act
//! for the decision's approver id, and refused once the map no longer does,
//! so an approval whose approver was removed between the decision and its
//! execution gets no execution permit. A runtime with no access controller
//! has no map, and refuses.

use std::sync::Arc;

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
use meerkat_meta::{AccessControlConfig, AccessController, UnifiedRuntime};
use meerkat_mob::MobDefinition;
use meerkat_runtime::decision::{
    DecisionApproverCurrentness, DecisionError, DecisionRefusal, DurableDecisionService,
};
use serde_json::json;

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

async fn runtime(state: &std::path::Path, map: Option<&str>) -> UnifiedRuntime {
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
        .decision_ingresses(
            serde_json::from_value(json!({"ingresses": [
                {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
            ]}))
            .expect("ingress declarations"),
        );
    if let Some(map) = map {
        let config: AccessControlConfig = toml::from_str(map).expect("map");
        builder = builder.access_controller(AccessController::new(config).expect("controller"));
    }
    Box::pin(builder.build()).await.expect("runtime")
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
