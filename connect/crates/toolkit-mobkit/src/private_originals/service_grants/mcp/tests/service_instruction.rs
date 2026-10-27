//! Task selection control through the actual canonical access/source binder.
//! This is native grant issuance and source binding, not native input admission.
use super::*;

#[tokio::test]
async fn service_instruction_task_a_grant_cannot_bind_fresh_b() {
    let dir = directory();
    let path = dir.path().join("grants");
    let owner = initialized(&path);
    let selected = selection(&config());
    let adapter = selected_adapter(owner, &dir.path().join("originals"), &selected).await;
    let mut approved = decision();
    approved.permission.lifetime.scope = LifetimeScope::Task { task: Id::new("old-a").unwrap() };
    let observed = apply(&adapter, approved, selected).await.unwrap();
    assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
    let lineage = adapter.service_grant_lineage(&observed.grant, &original(&adapter, "old-a")).await.unwrap();
    let reference = adapter.retain_event(private_event("fresh-b")).await.unwrap();
    let before = std::fs::read(&path).unwrap();
    let mut candidate = claims("fresh-b", &lineage[0]).candidate().clone();
    candidate.authority_basis = WorkAuthorityBasis::GrantLineage { lineage };
    let association = InputAuthorityAssociation::new(candidate).unwrap();
    let refused = adapter.bind_access_source(&reference, &observed.grant, association.clone(), source(&association)).await;
    assert!(matches!(refused, Err(PrivateOriginalError::Denied)));
    assert!(adapter.binding_evidence(&reference).await.unwrap().is_none());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
