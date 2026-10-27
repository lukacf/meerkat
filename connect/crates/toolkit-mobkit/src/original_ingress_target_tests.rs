//! Both directions of task-child isolation for every selected target. These
//! controls use native persisted grants and the source owner, not dispatch.
use super::*;
use meerkat_authorization_contracts::{constraints::DelegationDepth,
    work_association::{GrantTarget, TargetGrantLineage}};

fn targeted(owner: &ToolkitOriginalOwner, base: &GrantLineageRef, label: &str) -> InputAuthorityAssociation {
    let targets = [
        crate::installed_host::operator_setup::setup_targets::find_target(),
        crate::installed_host::acquisition::ConfiguredHelperReportCapability::target(),
        crate::installed_host::acquisition::ConfiguredHelperReportCapability::account_target(),
        crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target(),
        GrantTarget::McpServer { server: "selected-setup".into() },
    ];
    let child = |suffix: &str| owner.grants().issue_child(&principal("admin"), base,
        id(&format!("{label}-{suffix}")), principal("agent"), ExecutionRestrictions {
            delegation_depth: DelegationDepth::remaining(0), ..ExecutionRestrictions::unrestricted()
        }).unwrap();
    let mut entries = targets.into_iter().enumerate().map(|(index, target)| TargetGrantLineage {
        target, lineage: vec![base.clone(), child(&index.to_string())],
    }).collect::<Vec<_>>();
    entries.sort_by(|a, b| a.target.cmp(&b.target));
    let mut c = claims(label, base).candidate().clone();
    c.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(vec![base.clone(), child("catalog")]), targets: entries,
    };
    InputAuthorityAssociation::new(c).unwrap()
}

#[tokio::test]
async fn every_target_child_refuses_standing_or_other_task_reuse_in_both_rebuild_orders() {
    for index in 0..6 {
        let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("grants");
        let owner = initialized(&path); let base = base(&owner);
        let association = targeted(&owner, &base, "task-targets");
        let held = owner.bind_issued_access_async(association.clone(), source(&association),
            &principal("user"), &principal("frontend"), Some(&principal("admin")), |_| async { Ok(None) }).await.unwrap();
        let selected = association.candidate().authority_basis.grant_lineages().nth(index).unwrap().to_vec();
        let mut standing = claims("standing-reuse", &base).candidate().clone();
        standing.authority_basis = WorkAuthorityBasis::GrantLineage { lineage: selected.clone() };
        let standing = InputAuthorityAssociation::new(standing).unwrap();
        assert!(matches!(owner.bind_issued_access_async(standing.clone(), source(&standing),
            &principal("user"), &principal("frontend"), None, |_| async { Ok(None) }).await, Err(OriginalWriterError::Conflict)));
        let other = targeted(&owner, &base, "other-task");
        let mut c = other.candidate().clone();
        let WorkAuthorityBasis::TargetedGrantLineages { base: catalog, targets } = &mut c.authority_basis else { unreachable!() };
        // Reuse the exact selected native child without changing its path.
        if index == 0 { *catalog = Some(selected); } else { targets[index - 1].lineage = selected; }
        let other = InputAuthorityAssociation::new(c).unwrap();
        assert!(matches!(recover(&owner, &other), Err(OriginalWriterError::Conflict)));
        drop(held); drop(owner);
        let owner = restored(&path);
        owner.bind_issued_access_async(standing.clone(), source(&standing), &principal("user"),
            &principal("frontend"), None, |_| async { Ok(None) }).await.unwrap();
        assert!(matches!(recover(&owner, &association), Err(OriginalWriterError::Conflict)));
    }
}
