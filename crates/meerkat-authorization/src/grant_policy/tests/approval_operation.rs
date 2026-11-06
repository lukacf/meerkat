//! An approval-carried grant bound to one exact MCP operation: a call
//! resolves it only with the exact request's digest its owner states, and a
//! call whose digest differs, or whose owner states none, is refused at
//! preparation, before any entry, so nothing is spent.
use super::*;
use crate::grants::{
    ChildApprovalRule, GrantApproval, GrantChildTerms, GrantDecisionAssurance, GrantDecisionTier,
    GrantDeviceTrust, GrantOperationBinding, grant_child_terms_digest, mcp_operation_binding,
};
use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
use meerkat_core::approval::decision::{
    ApproverId, DecisionId, DecisionKind, DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::authorization::{ToolAuthorizationFacts, ToolAuthorizationTarget};
use meerkat_core::tool_execution::ToolOperationTarget;

/// Admits every original of the fixture's exact execution scope.
struct AnyAdmitted;
impl AdmittedWorkPolicyOwner for AnyAdmitted {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if binding.facts().execution_scope != scope() {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 1_000,
        })
    }
}

/// Maps a dispatched MCP operation to its raw operation on the server's own
/// domain.
struct McpOperations;
impl OperationPolicyOwner for McpOperations {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let AuthorizationOperation::Tool(ToolAuthorizationFacts {
            target: ToolAuthorizationTarget::Dispatcher(plan),
            ..
        }) = &binding.facts().operation
        else {
            return Err(denied().into());
        };
        let Some(ToolOperationTarget::McpServerOperation {
            server,
            raw_operation,
            ..
        }) = single_tool_operation_target(plan)?
        else {
            return Err(denied().into());
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![tuple(raw_operation, &format!("mcp-{server}"))],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 800,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

const SERVER: &str = "alpha";
const TOOL: &str = "query";

fn arguments() -> serde_json::Value {
    serde_json::json!({"sql": "select name from customers where id = 7", "limit": 1})
}

/// The digest the MCP router states for a call of `TOOL` on `SERVER` with
/// `arguments`.
fn digest_of(arguments: &serde_json::Value) -> Sha256Digest {
    meerkat_core::mcp_request_digest(SERVER, TOOL, arguments).expect("canonical request")
}

/// The exact operation the approval binds: `TOOL` on `SERVER` with
/// `arguments()`.
fn bound_operation() -> GrantOperationBinding {
    mcp_operation_binding(
        GrantTarget::McpServer {
            server: SERVER.into(),
        },
        TOOL,
        digest_of(&arguments()),
    )
}

/// A dispatched call of `TOOL` on `SERVER` whose owner states `digest` (or
/// none) as its exact request's digest.
fn call_stating(digest: Option<Sha256Digest>) -> PreparedAuthorizationBinding {
    let entry = meerkat_core::ToolCatalogEntry::session_inline(
        Arc::new(meerkat_core::ToolDef {
            name: "mcp_tool".into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
            audience: Default::default(),
        }),
        true,
    );
    let witness = meerkat_core::ToolExecutionOwnerWitness::new(
        "execution-owner",
        "mcp_tool",
        meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
    )
    .expect("owner witness")
    .with_operation_target(ToolOperationTarget::McpServerOperation {
        server: SERVER.into(),
        connection_generation: 1,
        raw_operation: TOOL.into(),
        connector: meerkat_core::McpConnectorApproval::NoConnector,
        account: Arc::new(meerkat_core::OperationAccount::Unavailable),
    });
    let witness = match digest {
        Some(digest) => witness.with_request_digest(digest),
        None => witness,
    };
    let plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_mins(10),
                ),
            ])
            .expect("deadline chain"),
        )
        .expect("plan")
        .with_owner_witness(witness)
        .expect("witness");
    bind(AuthorizationOperation::Tool(ToolAuthorizationFacts {
        call_id: Arc::from("call-1"),
        name: "mcp_tool".into(),
        arguments: serde_json::value::RawValue::from_string("{}".to_string())
            .expect("raw arguments")
            .into(),
        target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
    }))
}

/// The owner's agent's delegation for `TOOL` on `SERVER`, under the owner's
/// child rule, and its approval-carried child for the executor, bound to
/// the exact operation, for the fixture's requester and work.
fn approved_lineage(fixture: &Fixture) -> Vec<GrantLineageRef> {
    let approver = ApproverId::new("data-owner").expect("approver id");
    let parent_restrictions = ExecutionRestrictions {
        actions: ExactRestriction::exact([action(TOOL)]),
        resource_domains: DomainRestriction::exact([domain(&format!("mcp-{SERVER}"))]),
        delegation_depth: DelegationDepth::remaining(1),
        ..ExecutionRestrictions::unrestricted()
    };
    let parent = fixture
        .grants
        .issue_root_with_child_rule(
            &who("root"),
            id("offer-parent"),
            who("owner-agent"),
            Some(who("subject")),
            parent_restrictions.clone(),
            ChildApprovalRule {
                approver_id: approver.clone(),
                accepts_device_trust: GrantDeviceTrust::Refused,
            },
        )
        .expect("ruled parent");
    let requested = ExecutionRestrictions {
        actions: ExactRestriction::exact([action(TOOL)]),
        lifetime: LifetimeRestriction::window(100, 300),
        delegation_depth: DelegationDepth::remaining(0),
        ..ExecutionRestrictions::unrestricted()
    };
    let effective = DerivedChildRestrictions::new(parent_restrictions, requested.clone())
        .expect("attenuation")
        .effective()
        .clone();
    let association = fixture.association();
    let candidate = association.candidate();
    let terms = GrantChildTerms {
        parent: id("offer-parent"),
        issuer: who("owner-agent"),
        grantee: who("executor"),
        represented_subject: Some(who("subject")),
        actions: effective.actions,
        resource_domains: effective.resource_domains,
        processors: effective.processors,
        audiences: effective.audiences,
        window_ms: 200,
        operation: Some(bound_operation()),
        uses: 1,
        requester: candidate.requester.clone(),
        work: candidate.original_work.clone(),
        review_before_return: None,
    };
    let approval = GrantApproval {
        decision_id: DecisionId::derive(
            &DecisionKind::new("grant.child").expect("kind"),
            &DecisionSubjectKey::new("offer-1").expect("subject key"),
        ),
        approver_id: approver,
        tier: GrantDecisionTier::Human,
        assurance: GrantDecisionAssurance::PersonAuthenticated,
        request_digest: grant_child_terms_digest(&terms).expect("terms digest"),
        requester: crate::grants::dsl::GrantPrincipal::new(candidate.requester.clone())
            .expect("requester"),
        work: candidate.original_work.clone(),
        operation: Some(bound_operation()),
        uses: 1,
        review_before_return: None,
    };
    let child = fixture
        .grants
        .issue_approval_child(
            &who("owner-agent"),
            &parent,
            id("approved-child"),
            who("executor"),
            requested,
            approval,
        )
        .expect("approval child");
    vec![parent, child]
}

/// The fixture's work, whose one target is `SERVER`, on the approved
/// lineage, with no base.
fn approved_work(
    fixture: &Fixture,
    lineage: Vec<GrantLineageRef>,
) -> (Arc<GrantBackedWorkPolicy>, WorkAuthorizationContext) {
    let mut candidate = fixture.association().candidate().clone();
    candidate.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: vec![TargetGrantLineage {
            target: GrantTarget::McpServer {
                server: SERVER.into(),
            },
            lineage,
        }],
    };
    candidate.admitted_ceiling = ExecutionRestrictions::unrestricted();
    let association = InputAuthorityAssociation::new(candidate).expect("association");
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        fixture.grants.clone(),
        Arc::new(AnyAdmitted),
        Arc::new(McpOperations),
    ));
    let context = fixture.context(&policy, &association);
    (policy, context)
}

fn prepares(context: &WorkAuthorizationContext, binding: &PreparedAuthorizationBinding) -> bool {
    context.authorization().prepare(binding).is_ok()
}

fn denied_at_preparation(
    context: &WorkAuthorizationContext,
    binding: &PreparedAuthorizationBinding,
) -> bool {
    matches!(
        context.authorization().prepare(binding).map(|_| ()),
        Err(meerkat_core::OperationAuthorizationError::Refused(refused))
            if refused.kind() == OperationRefusalKind::Denied
    )
}

#[test]
fn a_call_whose_digest_differs_is_refused_before_entry_and_spends_nothing() {
    let fixture = Fixture::new();
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    let mut other = arguments();
    other["sql"] = serde_json::json!("select * from customers");
    let differing = call_stating(Some(digest_of(&other)));
    assert!(
        denied_at_preparation(&context, &differing),
        "a call of another request is refused at its preparation, before any entry"
    );
    // The same arguments with their members in another order and their
    // numbers in another form are the same request.
    let reordered: serde_json::Value =
        serde_json::from_str(r#"{"limit": 1.0, "sql": "select name from customers where id = 7"}"#)
            .expect("arguments");
    assert_eq!(digest_of(&reordered), digest_of(&arguments()));
    // Nothing was spent: the approved request itself still prepares, as
    // often as it is prepared again, until an entry spends its one use.
    for _ in 0..2 {
        assert!(prepares(
            &context,
            &call_stating(Some(digest_of(&reordered)))
        ));
    }
    assert!(denied_at_preparation(&context, &differing));
}

#[test]
fn an_owner_without_a_digest_never_matches_a_bound_grant() {
    let fixture = Fixture::new();
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    assert!(
        denied_at_preparation(&context, &call_stating(None)),
        "a call whose owner states no request digest never uses a grant bound to one exact request"
    );
    assert!(prepares(
        &context,
        &call_stating(Some(digest_of(&arguments())))
    ));
}
