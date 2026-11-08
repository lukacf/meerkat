//! Rows of an approved work's one reply to its requester: what
//! `reply_to_requester` answers says what happened to the reply.

use meerkat_core::authorization::{
    OperationAuthorizationError, OperationReviewTier, PreparedAuthorizationBinding,
    PreparedOperationAuthorization,
};
use meerkat_core::types::ToolCallView;
use meerkat_core::{AgentToolDispatcher, ToolDispatchContext};

use super::binding::resolution;
use super::*;

const ANSWER: &str = "revenue by region, last quarter: north 4.2, south 3.1";
const ANOTHER_ANSWER: &str = "the numbers are not available";

/// A prepared operation that admits its entry and spends nothing.
struct Admitted;

impl PreparedOperationAuthorization for Admitted {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }

    fn spend_at_entry(&self) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
}

/// A work owner that admits every operation: these rows are about the
/// reply's answers, not its authorization.
struct AdmitsAll;

impl meerkat_core::WorkAuthorization for AdmitsAll {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        Ok(Arc::new(Admitted))
    }
}

/// A dispatch in the approved work of `decision`: the work its terms name,
/// for their requester, run by its branch.
fn approved_work(fixture: &Fixture, decision: &DecisionId) -> ToolDispatchContext {
    let request = GrantChildRequest::of(&fixture.stored(decision)).expect("a grant.child request");
    let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: request.terms.requester.clone(),
        ingress_actor: agent("decision-ingress"),
        represented_subject: request.terms.represented_subject.clone(),
        original_authentication: evidence("authentication"),
        logical_executor: request.terms.grantee.clone(),
        target: NativeWorkTarget {
            logical_owner: agent("native-owner"),
            logical_runtime: id("branch-runtime"),
            context: id("branch-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: requester(),
            },
        },
        original_work: request.terms.work.clone(),
        root_event: evidence("approved-work"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: SERVER.into(),
                },
                lineage: fixture.offering.clone(),
            }],
        },
        controller_grant_lineage: Vec::new(),
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: realm(),
            ingress: ingress_domain(),
            occurrence_scope: id("approved-work-occurrence"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("the approved work's claims");
    let origin = GovernedPeerOrigin::from_associations([&association]).expect("origin");
    let work = meerkat_core::WorkAuthorizationContext::new(
        Arc::new(AdmitsAll),
        meerkat_core::OperationExecutionScope::Domain,
    )
    .with_governed_send_origin(Some(origin.to_send_origin()));
    ToolDispatchContext::default().with_work_authorization(Some(work))
}

/// What one `reply_to_requester` call of `text` answers in `context`: the
/// result's JSON, or the error's payload.
async fn reply(
    tools: &DecisionsToolDispatcher,
    context: &ToolDispatchContext,
    id: &str,
    text: &str,
) -> Result<serde_json::Value, serde_json::Value> {
    let args =
        serde_json::value::to_raw_value(&serde_json::json!({ "text": text })).expect("arguments");
    let call = ToolCallView {
        id,
        name: REPLY_TO_REQUESTER_TOOL,
        args: &args,
    };
    let plan = tools
        .resolve_execution_plan(call, context, &resolution())
        .expect("the reply resolves its plan");
    match tools
        .dispatch_resolved_with_context(call, context, &plan)
        .await
    {
        Ok(outcome) => {
            Ok(serde_json::from_str(&outcome.result.text_content()).expect("a JSON result"))
        }
        Err(error) => Err(error.to_error_payload()),
    }
}

/// A reply whose delivery failed was settled first: the answer says so, and
/// that it must be retried with exactly the same text. Another text is then
/// refused as another settled reply, never as one already delivered, and
/// the same text is delivered once.
#[tokio::test]
async fn a_reply_whose_delivery_failed_is_retried_with_its_text_and_never_called_delivered() {
    let fixture = Fixture::new();
    let decision = executed(&fixture).await;
    let tools = DecisionsToolDispatcher::new(fixture.grants.clone());
    let context = approved_work(&fixture, &decision);
    // The route cannot deliver the first reply now.
    fixture.route.delivery_failures.store(1, Ordering::SeqCst);
    let failed = reply(&tools, &context, "reply-1", ANSWER)
        .await
        .expect_err("the delivery failed");
    assert_eq!(failed["error"], "execution_failed", "{failed}");
    assert_eq!(
        failed["data"],
        serde_json::json!({"refusal": "reply_route_unavailable", "retry": "same_text"}),
        "the reply is settled, so only its own text is ever delivered: {failed}"
    );
    assert!(fixture.route.delivered().is_empty());
    // Another text: the settled reply is not this one.
    let another = reply(&tools, &context, "reply-2", ANOTHER_ANSWER)
        .await
        .expect_err("another text is refused");
    assert_eq!(another["error"], "execution_failed", "{another}");
    assert_eq!(
        another["data"],
        serde_json::json!({"refusal": "reply_already_settled"}),
        "{another}"
    );
    assert!(
        !another["message"]
            .as_str()
            .unwrap_or_default()
            .contains("delivered"),
        "nothing was delivered, and the answer never says it was: {another}"
    );
    assert!(fixture.route.delivered().is_empty());
    // The same text again is delivered, once.
    let delivered = reply(&tools, &context, "reply-3", ANSWER)
        .await
        .expect("the same text is delivered");
    assert_eq!(delivered["delivered"], true, "{delivered}");
    let again = reply(&tools, &context, "reply-4", ANSWER)
        .await
        .expect("the same text again answers the first delivery");
    assert_eq!(again["delivery"], delivered["delivery"]);
    assert_eq!(
        fixture.route.delivered(),
        vec![ANSWER.to_owned()],
        "one reply reached the requester"
    );
}
