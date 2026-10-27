//! Real ApprovalService decisions and the existing operator/native owner.
//! These controls do not run a Console presenter, MCP server or agent input.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::super::tests::{request, service_proposal};
use super::*;
use crate::private_originals::service_grants::catalog::CatalogDecisionKind;
use crate::private_originals::{
    PrivateServiceMcpAccessDecision, PrivateServiceMcpLiveAttachment, ServiceMcpSelection,
};
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::{ApprovalPrincipalId, ApprovalService, SessionId};
use meerkat_toolkit::authority::{LifetimeScope, RoleLifetime};
use std::collections::BTreeSet;
use std::path::Path;
use toolkit_store::StoreScope;

#[derive(Clone)]
struct Inputs {
    members: Vec<(
        PrivateServiceMcpAccessDecision,
        ServiceMcpSelection,
        ApprovalRequest,
    )>,
    catalog: ServiceCatalogDecision,
    request: ApprovalRequest,
}

impl Inputs {
    fn new(setup: &OperatorSetup) -> Self {
        let (base, _, mut request) = service_proposal(setup);
        let original = EvidenceId::new("fresh-b").unwrap();
        let session = SessionId::new();
        let member = &setup.native.members[0];
        let destination = NativeWorkTarget {
            logical_owner: member.logical_owner.clone(),
            logical_runtime: EvidenceId::new(
                meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string(),
            )
            .unwrap(),
            context: EvidenceId::new(session.to_string()).unwrap(),
            context_generation: member.context_generation,
            audience: member.audience.clone(),
        };
        request.proposed_action.body.as_mut().unwrap()["member_tool_policy"] = serde_json::json!({
            "tools": ["alpha_read", "beta_read"],
            "catalog": "r0",
            "scope": "review-context integrity only; policy publication is covered separately"
        });
        let members = ["alpha", "beta"]
            .into_iter()
            .map(|name| {
                let config = meerkat_core::McpServerConfig::stdio(
                    name,
                    "not-executed",
                    vec![],
                    Default::default(),
                );
                let selection = ServiceMcpSelection::no_connector(&config).unwrap();
                let mut decision = base.clone();
                decision.decision =
                    member_decision_id(&original, Some(&Id::new(name).unwrap())).unwrap();
                decision.permission.connection = Id::new(name).unwrap();
                decision.permission.resource = Id::new(name).unwrap();
                decision.permission.lifetime.expires_at_ms = Some(u64::MAX);
                decision.live_attachment = Some(PrivateServiceMcpLiveAttachment {
                    destination: destination.clone(),
                });
                decision.connection_revision =
                    toolkit_package::Revision::new(digest(&canonical_json(&config).unwrap()))
                        .unwrap();
                decision.evidence.resource_id = original.clone();
                decision.evidence.sha256 =
                    digest(&canonical_json(&request.proposed_action.body).unwrap());
                (decision, selection, request.clone())
            })
            .collect();
        let catalog = ServiceCatalogDecision {
            kind: CatalogDecisionKind::PublicServiceCatalogV1,
            route: base.route,
            decision: member_decision_id(&original, None).unwrap(),
            evidence: crate::private_originals::PrivateSourceEvidence {
                resource_id: original.clone(),
                revision: EvidenceId::new("public-service-batch-v1").unwrap(),
                sha256: digest(&canonical_json(&request.proposed_action.body).unwrap()),
            },
            session,
            original,
            lifetime: RoleLifetime {
                scope: LifetimeScope::Task {
                    task: Id::new("fresh-b").unwrap(),
                },
                expires_at_ms: Some(u64::MAX),
            },
        };
        Self {
            members,
            catalog,
            request,
        }
    }

    fn prepare(
        &self,
        setup: &OperatorSetup,
    ) -> Result<ServiceMcpParentBatchReview, HostLifetimeError> {
        let members = self
            .members
            .iter()
            .map(|(decision, selection, request)| {
                setup.prepare_service_mcp_parent_review(
                    decision.clone(),
                    selection.clone(),
                    request.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        setup.prepare_service_mcp_parent_batch_review(
            members,
            self.catalog.clone(),
            self.request.clone(),
        )
    }
}

fn approve(request: &ApprovalRequest) -> ApprovalRecord {
    let service = ApprovalService::new();
    let pending = service.request(request.clone()).unwrap();
    service
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-protected-batch-presenter").unwrap(),
            None,
            None,
        )
        .unwrap()
}

async fn records(setup: &OperatorSetup) -> Vec<StoredRecord> {
    setup
        .originals
        .list_records(
            &StoreScope {
                realm: setup.native.realm.to_string(),
                owner: setup.lifetime.selected_policy().deployment.to_string(),
            },
            None,
            64,
        )
        .await
        .unwrap()
}

async fn apply(
    setup: &OperatorSetup,
    review: ServiceMcpParentBatchReview,
    approval: &ApprovalRecord,
) -> Vec<GrantLineageRef> {
    let (members, catalog) = review.into_parts();
    let mut parents = Vec::new();
    for member in members {
        let expected = member.decision().clone();
        let ServiceMcpParentOutcome::Approved(approved) = setup
            .apply_service_mcp_parent_decision(member, approval.clone())
            .await
            .unwrap()
        else {
            panic!("actual batch member parent required")
        };
        assert!(approved.decision() == &expected);
        let (_, _, lineage, _) = approved.into_parts(&setup.source).unwrap();
        assert_eq!(lineage.len(), 1);
        parents.extend(lineage);
    }
    let approved = setup
        .apply_catalog_parent(catalog, approval.clone(), Arc::new(|| true))
        .await
        .unwrap();
    let (_, lineage, _) = approved.into_parts(&setup.source).unwrap();
    assert_eq!(lineage.len(), 1);
    parents.extend(lineage);
    parents
}

async fn unchanged(setup: &OperatorSetup, journal: &Path, bytes: &[u8], before: &[StoredRecord]) {
    assert_eq!(fs::read(journal).unwrap(), bytes);
    assert_eq!(records(setup).await, before);
}

#[tokio::test]
async fn one_real_batch_approval_issues_distinct_exact_parents_and_replays_without_writes() {
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let setup = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("batch-review", 1).unwrap(),
    )
    .unwrap();
    let journal = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let inputs = Inputs::new(&setup);
    let review = inputs.prepare(&setup).unwrap();
    let approved_request = review.request().clone();
    let approval = approve(&approved_request);
    assert_eq!(
        approved_request.proposed_action.body,
        approved_request.request_body
    );
    for member in review.members() {
        assert!(member.request() == &approved_request);
    }
    assert!(review.catalog().proposal.request == approved_request);
    let exact_set = review.set_sha256().to_owned();
    let parents = apply(&setup, review, &approval).await;
    assert_eq!(parents.len(), 3);
    assert_eq!(
        parents
            .iter()
            .map(|parent| parent.grant_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        3
    );
    let saved = records(&setup).await;
    assert_eq!(saved.len(), 3);
    for stored in &saved {
        let record = decode(stored, &stored.key).unwrap();
        assert!(record.approval == approval);
        assert!(matches!(&record.issued, ParentIssuance::Issued { lineage }
            if lineage.len() == 1 && parents.contains(&lineage[0])));
        assert_eq!(setup.parent_decision_key(&record).unwrap(), stored.key);
        let identity = record.batch_member.as_ref().unwrap();
        assert!(match identity {
            ServiceBatchMemberIdentity::PublicServicesV1 { set_sha256, index } =>
                set_sha256 == &exact_set && *index < 2,
            ServiceBatchMemberIdentity::PublicServiceCatalogV1 { set_sha256 } =>
                set_sha256 == &exact_set,
            _ => false,
        });
    }
    let bytes = fs::read(&journal).unwrap();
    assert_eq!(
        apply(&setup, inputs.prepare(&setup).unwrap(), &approval).await,
        parents
    );
    unchanged(&setup, &journal, &bytes, &saved).await;
    setup.finish().await.unwrap();
}

#[tokio::test]
async fn batch_approval_rejects_changed_member_catalog_instruction_policy_and_envelope_before_issuance()
 {
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let setup = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("batch-review", 1).unwrap(),
    )
    .unwrap();
    let journal = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let inputs = Inputs::new(&setup);
    let approval = approve(inputs.prepare(&setup).unwrap().request());
    let bytes = fs::read(&journal).unwrap();
    let saved = records(&setup).await;
    for change in [
        "member_operations",
        "member_context",
        "catalog_expiry",
        "instruction",
        "policy",
        "envelope",
    ] {
        let mut changed = inputs.clone();
        match change {
            "member_operations" => {
                changed.members[0]
                    .0
                    .permission
                    .operations
                    .remove(&Id::new("list").unwrap());
            }
            "member_context" => {
                changed.members[1].2.proposed_action.body.as_mut().unwrap()["instruction"] =
                    "Publish a summary".into()
            }
            "catalog_expiry" => changed.catalog.lifetime.expires_at_ms = Some(u64::MAX - 1),
            "instruction" => {
                changed.request.proposed_action.body.as_mut().unwrap()["instruction"] =
                    "Publish a summary".into()
            }
            "policy" => {
                changed.request.proposed_action.body.as_mut().unwrap()["member_tool_policy"]["tools"] =
                    serde_json::json!(["alpha_write", "beta_read"])
            }
            "envelope" => {
                changed.request.requester = ApprovalPrincipalId::new("other-invocation").unwrap()
            }
            _ => unreachable!(),
        }
        let (members, catalog) = changed.prepare(&setup).unwrap().into_parts();
        for member in members {
            assert!(
                matches!(
                    setup
                        .apply_service_mcp_parent_decision(member, approval.clone())
                        .await,
                    Err(HostLifetimeError::Denied)
                ),
                "{change}"
            );
        }
        assert!(
            matches!(
                setup
                    .apply_catalog_parent(catalog, approval.clone(), Arc::new(|| true))
                    .await,
                Err(HostLifetimeError::Denied)
            ),
            "{change}"
        );
        unchanged(&setup, &journal, &bytes, &saved).await;
    }
    setup.finish().await.unwrap();
}

#[tokio::test]
async fn batch_parent_rejects_wrong_set_index_and_catalog_identity_before_issuance() {
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let setup = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("batch-review", 1).unwrap(),
    )
    .unwrap();
    let journal = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let inputs = Inputs::new(&setup);
    let approval = approve(inputs.prepare(&setup).unwrap().request());
    let bytes = fs::read(&journal).unwrap();
    let saved = records(&setup).await;
    for change in ["set", "index", "out_of_bounds", "kind"] {
        let review = inputs.prepare(&setup).unwrap();
        let exact_set = review.set_sha256().to_owned();
        let (mut members, _) = review.into_parts();
        let mut member = members.remove(0);
        member.proposal.batch_member = Some(match change {
            "set" => ServiceBatchMemberIdentity::PublicServicesV1 {
                set_sha256: "0".repeat(64),
                index: 0,
            },
            "index" => ServiceBatchMemberIdentity::PublicServicesV1 {
                set_sha256: exact_set,
                index: 1,
            },
            "out_of_bounds" => ServiceBatchMemberIdentity::PublicServicesV1 {
                set_sha256: exact_set,
                index: 2,
            },
            "kind" => ServiceBatchMemberIdentity::PublicServiceCatalogV1 {
                set_sha256: exact_set,
            },
            _ => unreachable!(),
        });
        assert!(
            matches!(
                setup
                    .apply_service_mcp_parent_decision(member, approval.clone())
                    .await,
                Err(HostLifetimeError::InvalidSelection)
            ),
            "{change}"
        );
        unchanged(&setup, &journal, &bytes, &saved).await;
    }
    let review = inputs.prepare(&setup).unwrap();
    let set_sha256 = review.set_sha256().to_owned();
    let (_, mut catalog) = review.into_parts();
    catalog.proposal.batch_member = Some(ServiceBatchMemberIdentity::PublicServicesV1 {
        set_sha256,
        index: 0,
    });
    assert!(matches!(
        setup
            .apply_catalog_parent(catalog, approval, Arc::new(|| true))
            .await,
        Err(HostLifetimeError::InvalidSelection)
    ));
    unchanged(&setup, &journal, &bytes, &saved).await;
    setup.finish().await.unwrap();
}

#[tokio::test]
async fn batch_preparation_rejects_cross_task_destination_route_and_owner_members() {
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let setup = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("batch-review", 1).unwrap(),
    )
    .unwrap();
    let (_other_directory, other_selection) = crate::installed_host::tests::fixture();
    let other = OperatorSetup::open(
        other_selection.clone(),
        OperatorOwnerStartup::fresh("batch-review", 1).unwrap(),
    )
    .unwrap();
    let journal = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let other_journal =
        state_directory(&other_selection, &other.native, NativeOwnerStartup::Restore)
            .unwrap()
            .join("grants.jsonl");
    let bytes = fs::read(&journal).unwrap();
    let other_bytes = fs::read(&other_journal).unwrap();
    let saved = records(&setup).await;
    let other_saved = records(&other).await;
    let inputs = Inputs::new(&setup);
    for change in ["task", "catalog_task", "destination", "catalog_route"] {
        let mut changed = inputs.clone();
        match change {
            "task" => {
                changed.members[1].0.permission.lifetime.scope = LifetimeScope::Task {
                    task: Id::new("fresh-c").unwrap(),
                }
            }
            "catalog_task" => {
                changed.catalog.original = EvidenceId::new("fresh-c").unwrap();
                changed.catalog.lifetime.scope = LifetimeScope::Task {
                    task: Id::new("fresh-c").unwrap(),
                };
            }
            "destination" => changed.catalog.session = SessionId::new(),
            "catalog_route" => changed.catalog.route = Id::new("other-route").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            matches!(
                changed.prepare(&setup),
                Err(HostLifetimeError::InvalidSelection)
            ),
            "{change}"
        );
    }
    let mut mixed = Vec::new();
    for (index, (decision, selected, request)) in inputs.members.iter().enumerate() {
        let owner = if index == 0 { &setup } else { &other };
        mixed.push(
            owner
                .prepare_service_mcp_parent_review(
                    decision.clone(),
                    selected.clone(),
                    request.clone(),
                )
                .unwrap(),
        );
    }
    assert!(matches!(
        setup.prepare_service_mcp_parent_batch_review(mixed, inputs.catalog, inputs.request),
        Err(HostLifetimeError::WrongOwner)
    ));
    unchanged(&setup, &journal, &bytes, &saved).await;
    unchanged(&other, &other_journal, &other_bytes, &other_saved).await;
    setup.finish().await.unwrap();
    other.finish().await.unwrap();
}

#[tokio::test]
async fn singleton_parent_keeps_legacy_decision_bytes_and_approval_identity() {
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let setup = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("batch-review", 1).unwrap(),
    )
    .unwrap();
    let journal = state_directory(&selection, &setup.native, NativeOwnerStartup::Restore)
        .unwrap()
        .join("grants.jsonl");
    let proposal = setup
        .prepare_parent_review(
            Id::new("console").unwrap(),
            ExecutionRestrictions::unrestricted(),
            request(),
        )
        .unwrap();
    let approval = approve(proposal.request());
    let record = setup
        .parent_decision_record(&proposal, approval.clone())
        .unwrap();
    assert!(record.batch_member.is_none());
    assert!(encode(&record).unwrap().body.get("batch_member").is_none());
    let expected_body = canonical_json(&(
        &record.route,
        record.purpose,
        &record.setup_request,
        &record.operator,
        &record.application,
        &record.represented_subject,
        &record.approval,
        &record.requested,
    ))
    .unwrap();
    assert_eq!(decision_body(&record).unwrap(), expected_body);
    let expected_key = format!(
        "operator-parent-{}",
        digest(
            &canonical_json(&(
                "toolkit.operator-application-parent",
                &record.route,
                &record.approval.approval_id,
            ))
            .unwrap()
        )
    );
    assert_eq!(setup.parent_decision_key(&record).unwrap().id, expected_key);
    let OperatorParentOutcome::Selected { lineage, .. } = setup
        .apply_parent_decision(proposal.clone(), approval.clone())
        .await
        .unwrap()
    else {
        panic!("actual singleton parent required")
    };
    assert_eq!(lineage.len(), 1);
    assert_eq!(lineage[0].grant_id.as_str(), expected_key);
    let bytes = fs::read(&journal).unwrap();
    let saved = records(&setup).await;
    assert!(
        matches!(setup.apply_parent_decision(proposal, approval).await.unwrap(),
        OperatorParentOutcome::Selected { lineage: replay, .. } if replay == lineage)
    );
    unchanged(&setup, &journal, &bytes, &saved).await;
    setup.finish().await.unwrap();
}
