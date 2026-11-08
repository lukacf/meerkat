//! A Console withdraw of a durable decision is a console change: one whose
//! caller's login ended while it waited is refused with `caller_not_current`
//! right before it is made, and nothing is withdrawn.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionRenderer, DecisionRendering, DecisionRequestBody,
    DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use serde_json::json;

use super::console_decide_holds::ConsoleDecideHold;
use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{
    console_rpc_as, durable_fences, human_claims, now_seconds, token,
};
use super::{ConsoleJsonState, console_json_router_with_state};

/// A withdraw whose caller's sessions are revoked while it waits is refused
/// with `caller_not_current` right before it is made, and the decision stays
/// open.
#[tokio::test]
async fn a_console_withdraw_rechecks_its_caller_before_it_is_made() {
    let (state_dir, fences) = durable_fences();
    let decisions = crate::durable_decisions::RuntimeDecisions::in_state_dir(
        state_dir.path(),
        meerkat_core::RealmId::parse("team").expect("realm"),
        crate::durable_decisions::DecisionIngressDeclarations::default(),
        Arc::new(crate::runtime::InMemoryMetadataStore::new()),
        "decisions",
        true,
    );
    let requester = DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::PersonalAgent,
            "builder",
            TrustDomainId::new("example.org").expect("domain"),
        )
        .expect("principal"),
    )
    .expect("key");
    let held = decisions
        .owner()
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("release").expect("kind"),
                DecisionSubjectKey::new("a").expect("subject"),
                DecisionOperationId::new("operation:a").expect("operation"),
                Sha256Digest::of(br#"{"build":7}"#),
                DecisionRequestBody::from_value(&json!({"build": 7})).expect("body"),
                requester,
                ApproverId::new("team:owners").expect("approver id"),
                u64::MAX / 2,
                DecisionRendering::new("Release build 7.").expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold");
    let access = crate::access::AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller");
    let app = console_json_router_with_state(ConsoleJsonState {
        session_fences: fences.clone(),
        access: Some(access),
        durable_decisions: decisions.clone(),
        ..state()
    });
    let bob = token(human_claims(), Some(now_seconds() + 3600));
    let decision_id = held.decision_id.to_string();

    let hold = ConsoleDecideHold::install(&decision_id);
    let withdraw = tokio::spawn({
        let app = app.clone();
        let decision_id = decision_id.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bob),
                "meta/decisions/withdraw",
                json!({"decision_id": decision_id}),
            )
            .await
        }
    });
    tokio::time::timeout(Duration::from_secs(30), hold.reached())
        .await
        .expect("the withdraw reached the hold");
    fences
        .revoke_sessions(BOB, std::time::SystemTime::now() + Duration::from_secs(5))
        .expect("revoke");
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), withdraw)
        .await
        .expect("the withdraw answered")
        .expect("the withdraw task");
    assert_eq!(answer["error"]["code"], json!(-32056), "{answer:#}");
    assert_eq!(
        answer["error"]["data"]["kind"],
        json!("caller_not_current"),
        "{answer:#}"
    );
    assert_eq!(
        decisions
            .owner()
            .get(&held.decision_id)
            .expect("row")
            .state
            .phase,
        DecisionPhase::Open,
        "nothing was withdrawn"
    );
}
