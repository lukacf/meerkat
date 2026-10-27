//! A host's governed composition over the real grant owner and the real
//! `memory_search` owner, as the factory mounts it behind a tool gateway:
//! the read of the session memory that the dispatcher's own plan names
//! selects its own grant lineage through the native fenced dispatch. The
//! search is denied without a lineage selected for exactly that memory
//! scope, even though the work's base lineage is unrestricted, and runs with
//! one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, GrantBackedWorkPolicy, OperationPolicyOwner, WorkOwnerAllowance,
    single_tool_operation_target,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DomainRestriction, ExactRestriction, ExecutionRestrictions,
    ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate, NativeWorkTarget,
    OriginalWorkRef, QualifiedIngressNamespace, TargetGrantLineage, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationError, OperationRefusalKind, OperationReviewTier,
    PreparedAuthorizationBinding, ToolAuthorizationFacts, ToolAuthorizationTarget,
};
use meerkat_core::connection::RealmId;
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::memory::{
    MemoryIndexRequest, MemoryIndexScope, MemoryMetadata, MemorySearchScope, MemorySource,
    MemoryStore, MessageRange,
};
use meerkat_core::types::{SessionId, ToolCallView};
use meerkat_core::{
    AgentToolDispatcher, MemoryOperation, MemoryScopeRef, PrincipalKind, PrincipalRef,
    ToolDispatchContext, ToolError, ToolOperationTarget, TrustDomainId,
};

fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, id, TrustDomainId::new("host-memory").unwrap()).unwrap()
}

fn operator() -> PrincipalRef {
    principal(PrincipalKind::Human, "operator")
}

fn ingress() -> PrincipalRef {
    principal(PrincipalKind::RuntimeHost, "host-ingress")
}

fn executor() -> PrincipalRef {
    principal(PrincipalKind::PersonalAgent, "executor")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: ingress(),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([7; 32]),
    }
}

/// The host's own action for a read of one session's memory.
fn memory_read() -> ActionRef {
    ActionRef {
        feature: "memory".into(),
        action: "read".into(),
    }
}

/// The host's own domain of one session's memory.
fn memory_domain(session: &SessionId) -> ResourceDomain {
    ResourceDomain {
        authority: operator(),
        namespace: format!("memory/{session}"),
    }
}

fn memory_target(session: &SessionId) -> GrantTarget {
    GrantTarget::Memory {
        scope: MemorySearchScope::for_session(session.clone()).scope_ref(),
    }
}

/// Admits the host's own work, whatever it selects: only the grants and the
/// operation owner below decide.
struct AnyAdmitted;
impl AdmittedWorkPolicyOwner for AnyAdmitted {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

/// The host maps the owner-produced read of one session's memory to its
/// own action on that session's memory domain, and nothing else.
struct HostMemoryReads;
impl OperationPolicyOwner for HostMemoryReads {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let AuthorizationOperation::Tool(ToolAuthorizationFacts {
            target: ToolAuthorizationTarget::Dispatcher(plan),
            ..
        }) = &binding.facts().operation
        else {
            return Err(OperationAuthorizationError::Unavailable);
        };
        let Some(ToolOperationTarget::MemoryOperation {
            scope: MemoryScopeRef::Session { session_id },
            operation: MemoryOperation::Read,
        }) = single_tool_operation_target(plan)?
        else {
            return Err(OperationAuthorizationError::Unavailable);
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: memory_read(),
                resource_domain: memory_domain(session_id),
                processor: ProcessorRef::Principal {
                    principal: executor(),
                },
                audience: association.candidate().target.audience.clone(),
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
            review_tier: OperationReviewTier::R1,
        })
    }
}

/// The host's work at `session`: an unrestricted base lineage and exactly
/// `targets`.
fn association(
    session: &SessionId,
    base: &GrantLineageRef,
    targets: Vec<(GrantTarget, GrantLineageRef)>,
) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: operator(),
        ingress_actor: ingress(),
        represented_subject: None,
        original_authentication: evidence("authentication"),
        logical_executor: executor(),
        target: NativeWorkTarget {
            logical_owner: principal(PrincipalKind::RuntimeHost, "session-owner"),
            logical_runtime: id("runtime"),
            context: id(&session.to_string()),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: operator(),
            },
        },
        original_work: OriginalWorkRef {
            authority: ingress(),
            work: id("work-1"),
        },
        root_event: evidence("event"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(vec![base.clone()]),
            targets: targets
                .into_iter()
                .map(|(target, lineage)| TargetGrantLineage {
                    target,
                    lineage: vec![lineage],
                })
                .collect(),
        },
        controller_grant_lineage: vec![base.clone()],
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("host-realm").unwrap(),
            ingress: ResourceDomain {
                authority: ingress(),
                namespace: "host-prompts".into(),
            },
            occurrence_scope: id("occurrence-1"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .unwrap()
}

fn governed(
    policy: &Arc<GrantBackedWorkPolicy>,
    association: InputAuthorityAssociation,
) -> ToolDispatchContext {
    let work = policy
        .work_context(Arc::from([association]), OperationExecutionScope::Domain)
        .unwrap();
    ToolDispatchContext::default().with_work_authorization(Some(work))
}

/// Resolve `call` on `root` under `context`, then dispatch the plan through
/// the native fenced dispatch, which prepares the operation's authorization.
async fn search(
    root: &Arc<dyn AgentToolDispatcher>,
    call: ToolCallView<'_>,
    context: &ToolDispatchContext,
) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
    let plan = meerkat_core::agent::resolve_tool_execution_plan_fenced(
        root,
        call,
        context,
        &meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    Duration::from_mins(10),
                ),
            ])
            .unwrap(),
        ),
    )
    .unwrap();
    meerkat_core::agent::dispatch_tool_execution_plan_fenced(root, call, context, &plan).await
}

fn is_denied(result: &Result<meerkat_core::ops::ToolDispatchOutcome, ToolError>) -> bool {
    matches!(result, Err(ToolError::AuthorizationRefused { refusal })
        if refusal.kind() == OperationRefusalKind::Denied)
}

#[tokio::test]
async fn a_memory_search_needs_the_lineage_selected_for_its_own_memory_scope() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: operator(),
                namespace: id("host-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        Arc::clone(&grants),
        Arc::new(AnyAdmitted),
        Arc::new(HostMemoryReads),
    ));
    let session = SessionId::new();
    let base = grants
        .issue_root(
            &operator(),
            id("base"),
            executor(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .unwrap();
    let memory = grants
        .issue_root(
            &operator(),
            id("session-memory-read"),
            executor(),
            None,
            ExecutionRestrictions {
                actions: ExactRestriction::exact([memory_read()]),
                resource_domains: DomainRestriction::exact([memory_domain(&session)]),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .unwrap();

    let store: Arc<dyn MemoryStore> = Arc::new(meerkat_memory::SimpleMemoryStore::new());
    store
        .index_scoped(
            MemoryIndexRequest::new(
                MemoryIndexScope::for_session(session.clone()),
                meerkat_core::MemoryIndexableContent::Indexable(
                    "The project codename is AURORA-7".into(),
                ),
                MemoryMetadata {
                    session_id: session.clone(),
                    source: MemorySource::Compaction {
                        source_range: MessageRange::single(7),
                    },
                    indexed_at: SystemTime::now(),
                },
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let root: Arc<dyn AgentToolDispatcher> = Arc::new(
        meerkat_core::ToolGatewayBuilder::new()
            .add_dispatcher(Arc::new(
                meerkat_memory::MemorySearchDispatcher::for_session(store, session.clone()),
            ))
            .build()
            .unwrap(),
    );
    let args =
        serde_json::value::RawValue::from_string(r#"{"query": "project codename"}"#.to_string())
            .unwrap();
    let call = ToolCallView {
        id: "call-memory",
        name: "memory_search",
        args: &args,
    };

    // The base lineage alone, unrestricted as it is, covers no memory read.
    let base_only = governed(&policy, association(&session, &base, Vec::new()));
    assert!(is_denied(&search(&root, call, &base_only).await));

    // A lineage selected for another session's memory covers only that one.
    let other_scope = governed(
        &policy,
        association(
            &session,
            &base,
            vec![(memory_target(&SessionId::new()), memory.clone())],
        ),
    );
    assert!(is_denied(&search(&root, call, &other_scope).await));

    // The lineage selected for exactly this session's memory: the search runs.
    let own_scope = governed(
        &policy,
        association(&session, &base, vec![(memory_target(&session), memory)]),
    );
    let outcome = search(&root, call, &own_scope).await.unwrap();
    assert!(!outcome.result.is_error);
    assert!(
        outcome.result.text_content().contains("AURORA-7"),
        "the search read this session's memory: {:?}",
        outcome.result
    );
}
