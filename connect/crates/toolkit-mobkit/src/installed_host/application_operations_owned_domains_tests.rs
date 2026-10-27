//! Actual native durable grants for distinct configured authorities and session
//! coordinates. No session runtime, model, provider or connector is executed.
use super::*;
use crate::original_ingress::{ToolkitOriginalOwner, tests::principal};
use meerkat_authorization::{
    clock::HostAuthorizationClock,
    grants::{ChildGrantIntent, ChildGrantOutcome, ChildGrantRefusal, ChildGrantValidity, FileGrantOwnerStore},
};
use meerkat_authorization_contracts::{
    constraints::{DomainBound, OperationRestrictionValues, RestrictionDimension, RestrictionFailure},
    evidence::EvidenceId,
};
use std::sync::Arc;

fn id(value: &str) -> EvidenceId { EvidenceId::new(value).unwrap() }
fn open() -> Result<ChildGrantValidity, meerkat_authorization::grants::GrantRefusal> {
    Ok(ChildGrantValidity {
        not_before_ms: 0,
        expires_at_ms: u64::MAX,
        deadline: meerkat_core::time_compat::Instant::now() + std::time::Duration::from_secs(60),
    })
}
fn check(bounds: &ExecutionRestrictions, values: &LocalOperationValues, now_ms: u64)
    -> Result<(), RestrictionFailure>
{
    bounds.check_bounds(OperationRestrictionValues {
        action: &values.action, resource_domain: &values.resource_domain,
        processor: &values.processor, audience: &values.audience, now_ms,
    })
}

#[test]
fn configured_authorities_keep_native_session_children_separate_across_restore() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("grants.jsonl");
    let source = crate::original_ingress::tests::initialized(&path);
    let grants = source.grants();
    let applications = [principal("configured-app-a"), principal("configured-app-b")];
    assert_ne!(applications[0], applications[1]);
    let sessions = [[SessionId::new(), SessionId::new()], [SessionId::new(), SessionId::new()]];
    let executor = principal("shared-executor");
    let audience = AudienceRef::Principal { principal: principal("shared-audience") };
    let mut retained = Vec::new();
    for (app_index, application) in applications.iter().enumerate() {
        // These are the real parent constructors. Only the domain dimension
        // expands beyond a session, and only within this configured authority.
        let parents = [
            configured_catalog_parent(application, &executor, &audience),
            configured_setup_parent(application, &executor, &audience),
            super::super::operator_setup::configured_report::report_parent(application, &executor, &audience),
            super::super::operator_setup::configured_report::account_parent(application, &executor, &audience),
            super::super::operator_setup::configured_report::public_parent(application, &executor, &audience),
        ];
        for (parent, action) in parents.iter().zip([
            "catalog-read", "find-and-install", "setup-report", "setup-account-request", "setup-public-service-request",
        ]) {
            assert_eq!(parent.actions, ExactRestriction::exact([ActionRef { feature: "toolkit".into(), action: action.into() }]));
            assert_eq!(parent.resource_domains, DomainRestriction::owned_by([application.clone()]));
            assert_eq!(parent.processors, ExactRestriction::exact([ProcessorRef::Principal { principal: executor.clone() }]));
            assert_eq!(parent.audiences, ExactRestriction::exact([audience.clone()]));
            assert_eq!(parent.lifetime, LifetimeRestriction::unrestricted());
            assert_eq!(parent.delegation_depth, DelegationDepth::remaining(1));
        }
        for (family, parent_bounds) in parents[..2].iter().enumerate() {
            let parent = grants.issue_root(&principal("admin"), id(&format!("app-{app_index}-family-{family}")),
                application.clone(), None, parent_bounds.clone()).unwrap();
            for (session_index, session) in sessions[app_index].iter().enumerate() {
                let operation = if family == 0 { catalog_read } else { find_and_install };
                let own = operation(application, &executor, &audience, session);
                let mut requested = parent_restrictions(own.clone());
                requested.delegation_depth = DelegationDepth::remaining(0);
                let intent = ChildGrantIntent {
                    issuer: application.clone(), parent_lineage: vec![parent.clone()],
                    id: id(&format!("child-{app_index}-{family}-{session_index}")),
                    grantee: executor.clone(), represented_subject: None, requested: requested.clone(),
                };
                let ChildGrantOutcome::Issued { lineage } = grants.issue_or_recover_child_under_exclusion(&intent, open).unwrap()
                    else { panic!("fresh child was not issued") };
                let resolved = grants.resolve_controller_lineage(&lineage, &executor, None).unwrap();
                assert_eq!(resolved.restrictions(), &requested);
                resolved.check_current().unwrap();
                let now = source.clock().now().unwrap().unix_ms;
                assert_eq!(check(resolved.restrictions(), &own, now), Ok(()));
                let same_app_other_session = operation(application, &executor, &audience, &sessions[app_index][1 - session_index]);
                let other_app = operation(&applications[1 - app_index], &executor, &audience, session);
                for excluded in [&same_app_other_session, &other_app] {
                    assert_eq!(check(resolved.restrictions(), excluded, now),
                        Err(RestrictionFailure::OutsideBounds { dimension: RestrictionDimension::ResourceDomain }));
                }
                let mut arbitrary_foreign_namespace = other_app.clone();
                arbitrary_foreign_namespace.resource_domain.namespace = "another-namespace".into();
                assert_eq!(check(parent_bounds, &arbitrary_foreign_namespace, now),
                    Err(RestrictionFailure::OutsideBounds { dimension: RestrictionDimension::ResourceDomain }));
                for (dimension, changed) in [
                    (RestrictionDimension::Action, LocalOperationValues { action: ActionRef { feature: "mcp".into(), action: "send".into() }, ..own.clone() }),
                    (RestrictionDimension::Processor, LocalOperationValues { processor: ProcessorRef::Principal { principal: principal("other-executor") }, ..own.clone() }),
                    (RestrictionDimension::Audience, LocalOperationValues { audience: AudienceRef::Principal { principal: principal("other-audience") }, ..own.clone() }),
                ] {
                    assert_eq!(check(resolved.restrictions(), &changed, now), Err(RestrictionFailure::OutsideBounds { dimension }));
                }
                let journal = std::fs::read(&path).unwrap();
                let grandchild = ChildGrantIntent {
                    issuer: executor.clone(), parent_lineage: lineage.clone(),
                    id: id(&format!("grandchild-{app_index}-{family}-{session_index}")),
                    ..intent.clone()
                };
                assert_eq!(grants.issue_or_recover_child_under_exclusion(&grandchild, open), Err(ChildGrantRefusal::Denied));
                assert_eq!(grants.issue_or_recover_child_under_exclusion(&intent, open),
                    Ok(ChildGrantOutcome::Recovered { lineage: lineage.clone() }));
                let foreign_intent = ChildGrantIntent { requested: ExecutionRestrictions {
                    resource_domains: DomainRestriction::exact([other_app.resource_domain.clone()]), ..requested.clone()
                }, ..intent.clone() };
                assert_eq!(grants.issue_or_recover_child_under_exclusion(&foreign_intent, open), Err(ChildGrantRefusal::Denied));
                assert_eq!(std::fs::read(&path).unwrap(), journal);
                let foreign_child = parent_bounds.conjoin(&foreign_intent.requested);
                assert!(matches!(foreign_child.resource_domains.bound(), DomainBound::Exact(values) if values.is_empty()));
                assert_eq!(check(&foreign_child, &own, now), Err(RestrictionFailure::OutsideBounds { dimension: RestrictionDimension::ResourceDomain }));
                assert_eq!(check(&foreign_child, &other_app, now), Err(RestrictionFailure::OutsideBounds { dimension: RestrictionDimension::ResourceDomain }));
                retained.push((intent, lineage, requested, own, same_app_other_session, other_app));
            }
        }
    }
    let journal = std::fs::read(&path).unwrap();
    let domain = source.ingress_domain().clone();
    drop(grants);
    drop(source);
    let restored = ToolkitOriginalOwner::restore(Arc::new(FileGrantOwnerStore::new(&path)), Arc::new(HostAuthorizationClock), domain).unwrap();
    for (intent, lineage, expected, own, other_session, other_app) in retained {
        let grants = restored.grants();
        assert_eq!(grants.issue_or_recover_child_under_exclusion(&intent, open), Ok(ChildGrantOutcome::Recovered { lineage: lineage.clone() }));
        let resolved = grants.resolve_controller_lineage(&lineage, &executor, None).unwrap();
        assert_eq!(resolved.restrictions(), &expected);
        let now = restored.clock().now().unwrap().unix_ms;
        assert_eq!(check(resolved.restrictions(), &own, now), Ok(()));
        for excluded in [&other_session, &other_app] {
            assert_eq!(check(resolved.restrictions(), excluded, now), Err(RestrictionFailure::OutsideBounds { dimension: RestrictionDimension::ResourceDomain }));
        }
    }
    assert_eq!(std::fs::read(&path).unwrap(), journal);
}
