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

/// The host's scripted answer for the person who decided an
/// approval-carried grant's decision, and how often it was asked.
struct Currentness {
    answer: std::sync::Mutex<crate::grants::ApprovalUseAnswer>,
    asked: std::sync::atomic::AtomicUsize,
}

impl Currentness {
    fn answering(answer: crate::grants::ApprovalUseAnswer) -> Arc<Self> {
        Arc::new(Self {
            answer: std::sync::Mutex::new(answer),
            asked: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn set(&self, answer: crate::grants::ApprovalUseAnswer) {
        *self.answer.lock().expect("answer") = answer;
    }

    fn asked(&self) -> usize {
        self.asked.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl crate::grants::ApprovalUseCurrentness for Currentness {
    fn approver_answer(&self, _approval: &GrantApproval) -> crate::grants::ApprovalUseAnswer {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.answer.lock().expect("answer")
    }
}

/// The approved call: `TOOL` on `SERVER` with exactly `arguments()`.
fn approved_call() -> PreparedAuthorizationBinding {
    call_stating(Some(digest_of(&arguments())))
}

/// A dispatched call whose owner states `target`, or no target.
fn call_with(target: Option<ToolOperationTarget>) -> PreparedAuthorizationBinding {
    let entry = meerkat_core::ToolCatalogEntry::session_inline(
        Arc::new(meerkat_core::ToolDef {
            name: "owned_tool".into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
            audience: Default::default(),
        }),
        true,
    );
    let witness = meerkat_core::ToolExecutionOwnerWitness::new(
        "execution-owner",
        "owned_tool",
        meerkat_core::ephemeral_tool_catalog_binding_fingerprint(&entry),
    )
    .expect("owner witness");
    let witness = match target {
        Some(target) => witness.with_operation_target(target),
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
        name: "owned_tool".into(),
        arguments: serde_json::value::RawValue::from_string("{}".to_string())
            .expect("raw arguments")
            .into(),
        target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
    }))
}

fn refused_as(
    result: Result<(), meerkat_core::OperationAuthorizationError>,
    kind: OperationRefusalKind,
) -> bool {
    matches!(result, Err(meerkat_core::OperationAuthorizationError::Refused(refused))
        if refused.kind() == kind)
}

#[test]
fn the_entry_spends_the_approved_use_once() {
    let currentness = Currentness::answering(crate::grants::ApprovalUseAnswer::Current);
    let fixture = Fixture::new_with(Some(currentness.clone()));
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    let prepared = context
        .authorization()
        .prepare(&approved_call())
        .expect("the approved call prepares");
    assert_eq!(currentness.asked(), 0, "preparing spends and asks nothing");
    prepared
        .spend_at_entry()
        .expect("the entry spends the one use");
    assert_eq!(
        currentness.asked(),
        1,
        "the decider's currentness is asked at the spend"
    );
    // The spend advanced the owner: the decision prepared before it refuses
    // to rebuild instead of spending again, and nothing prepares the spent
    // use any more.
    assert!(refused_as(
        prepared.spend_at_entry(),
        OperationRefusalKind::ReprepareRequired
    ));
    assert!(denied_at_preparation(&context, &approved_call()));
    // An operation whose lineage ends at no approval spends nothing.
    let plain = fixture.association();
    let (policy, _, _) = fixture.policy(plain.clone());
    let read = source(SourceAuthorizationUse::Read, "public");
    let prepared = fixture
        .context(&policy, &plain)
        .authorization()
        .prepare(&read)
        .expect("an ordinary read prepares");
    prepared
        .spend_at_entry()
        .expect("an ordinary entry spends nothing");
    assert_eq!(currentness.asked(), 1);
}

#[test]
fn a_spend_whose_approver_is_not_current_or_unsettled_spends_nothing() {
    let currentness = Currentness::answering(crate::grants::ApprovalUseAnswer::NotCurrent);
    let fixture = Fixture::new_with(Some(currentness.clone()));
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    let prepared = context
        .authorization()
        .prepare(&approved_call())
        .expect("prepares");
    assert!(
        refused_as(prepared.spend_at_entry(), OperationRefusalKind::Denied),
        "a decider no longer current refuses the entry"
    );
    currentness.set(crate::grants::ApprovalUseAnswer::Unsettled);
    let prepared = context
        .authorization()
        .prepare(&approved_call())
        .expect("nothing was spent: the use still prepares");
    assert!(matches!(
        prepared.spend_at_entry(),
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    currentness.set(crate::grants::ApprovalUseAnswer::Current);
    let prepared = context
        .authorization()
        .prepare(&approved_call())
        .expect("nothing was spent: the use still prepares");
    prepared.spend_at_entry().expect("a current decider spends");
    assert!(denied_at_preparation(&context, &approved_call()));
}

#[test]
fn an_owner_composed_without_a_currentness_owner_spends_nothing() {
    let fixture = Fixture::new();
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    let prepared = context
        .authorization()
        .prepare(&approved_call())
        .expect("prepares");
    assert!(matches!(
        prepared.spend_at_entry(),
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    assert!(prepares(&context, &approved_call()));
}

/// The approved work of `approved_work`, with `edit` applied to its claims.
fn approved_work_edited(
    fixture: &Fixture,
    lineage: Vec<GrantLineageRef>,
    edit: impl FnOnce(&mut InputAuthorityAssociationCandidate),
) -> WorkAuthorizationContext {
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
    edit(&mut candidate);
    let association = InputAuthorityAssociation::new(candidate).expect("association");
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        fixture.grants.clone(),
        Arc::new(AnyAdmitted),
        Arc::new(McpOperations),
    ));
    fixture.context(&policy, &association)
}

#[test]
fn reply_to_requester_is_denied_outside_an_approved_work() {
    let fixture = Fixture::new();
    let lineage = approved_lineage(&fixture);
    let reply = call_with(Some(ToolOperationTarget::RequesterReply));
    // In the approved work, Core's rule admits the reply: no grant of its
    // own and no operation owner's answer (this one maps only MCP calls).
    let (_policy, approved) = approved_work(&fixture, lineage.clone());
    assert!(prepares(&approved, &reply));
    // Its reply never spends or resolves the approval: the approved call
    // still prepares after any number of replies.
    assert!(prepares(&approved, &reply));
    assert!(prepares(&approved, &approved_call()));
    // An ordinary work is denied the reply.
    let plain = fixture.association();
    let (policy, _, _) = fixture.policy(plain.clone());
    assert!(denied_at_preparation(
        &fixture.context(&policy, &plain),
        &reply
    ));
    // So is a work that names the approval's lineage but is not its work:
    // another requester, another original, another executor, a contributor
    // or a base lineage.
    let base = fixture.lineage.clone();
    let edits: Vec<Box<dyn FnOnce(&mut InputAuthorityAssociationCandidate)>> = vec![
        Box::new(|candidate| candidate.requester = who("another-requester")),
        Box::new(|candidate| candidate.original_work.work = id("another-work")),
        Box::new(|candidate| candidate.logical_executor = who("another-executor")),
        Box::new(|candidate| {
            candidate.contributing_work = vec![OriginalWorkRef {
                authority: who("ingress"),
                work: id("contributor"),
            }];
        }),
        Box::new(move |candidate| {
            if let WorkAuthorityBasis::TargetedGrantLineages { base: selected, .. } =
                &mut candidate.authority_basis
            {
                *selected = Some(base);
            }
        }),
    ];
    for edit in edits {
        let context = approved_work_edited(&fixture, lineage.clone(), edit);
        assert!(denied_at_preparation(&context, &reply));
    }
}

#[test]
fn another_target_in_the_approved_work_is_denied() {
    let fixture = Fixture::new();
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    let another_server = ToolOperationTarget::McpServerOperation {
        server: "beta".into(),
        connection_generation: 1,
        raw_operation: TOOL.into(),
        connector: meerkat_core::McpConnectorApproval::NoConnector,
        account: Arc::new(meerkat_core::OperationAccount::Unavailable),
    };
    let peer = ToolOperationTarget::PeerSend {
        peer: meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(7)),
        kind: meerkat_core::PeerSendKind::Request,
    };
    let hold = ToolOperationTarget::DecisionHold {
        approver_id: ApproverId::new("data-owner").expect("approver id"),
    };
    for target in [
        another_server,
        peer,
        ToolOperationTarget::PeerDirectoryRead,
        hold,
    ] {
        assert!(denied_at_preparation(&context, &call_with(Some(target))));
    }
    assert!(prepares(&context, &approved_call()));
}

#[test]
fn a_builtin_without_a_target_in_the_approved_work_is_denied() {
    let fixture = Fixture::new();
    let lineage = approved_lineage(&fixture);
    let (_policy, context) = approved_work(&fixture, lineage);
    // No target: the base lineage, which the approved work has none of.
    assert!(denied_at_preparation(&context, &call_with(None)));
    // A memory write: its scope's own lineage, of which it has none either.
    let memory_write = ToolOperationTarget::MemoryOperation {
        scope: meerkat_core::MemoryScopeRef::Session {
            session_id: SessionId::from_uuid(uuid::Uuid::from_u128(3)),
        },
        operation: meerkat_core::MemoryOperation::Write,
    };
    assert!(denied_at_preparation(
        &context,
        &call_with(Some(memory_write))
    ));
}
