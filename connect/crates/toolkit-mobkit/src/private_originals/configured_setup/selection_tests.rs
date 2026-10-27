//! Canonical selection/recovery with actual native roots and children. The
//! target selection is a compiler fixture; the public producer has separate
//! controls. No helper model or account provider is invoked here.
use super::*;
use crate::private_originals::{ContinuationFixture, PrivateOriginalSubmit, PrivateOriginalSubmission};
use crate::installed_host::{acquisition::ConfiguredHelperReportCapability, operator_setup::AgentSetupCapabilityRequest};
use meerkat_authorization_contracts::constraints::{ActionRef, ExactRestriction, ProcessorRef};
use meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupTarget;
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

async fn captured(fixture: &ContinuationFixture, name: &str) -> PrivateOriginalSubmit {
    let content = "Find the connector needed for this original task".to_owned();
    let mut event = crate::original_ingress::tests::private_event(name);
    event.route = Id::new("console").unwrap(); event.content_sha256 = digest(content.as_bytes());
    let authentication = event.original_authentication.clone();
    let original = fixture.source.retain_event(event).await.unwrap();
    PrivateOriginalSubmit { original, content, transport_authentication: authentication }
}
fn capability(fixture: &ContinuationFixture, name: &str) -> CompiledAgentSetupCapability {
    let native = fixture.lifetime().selected_policy().native_host.as_ref().unwrap();
    let owner = fixture.source.local_operator_setup(fixture.lifetime().clone()).unwrap();
    let mut selected = owner.configure_agent_setup(&AgentSetupCapabilityRequest {
        helper: None,
        member: native.members[0].member.clone(), route: native.routes[0].id.clone(),
        issuance_id: EvidenceId::new(format!("{name}-find")).unwrap(), helper_targets: vec![],
    }).unwrap();
    let restrictions = ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef { feature: "toolkit".into(), action: "setup-report".into() }]),
        processors: ExactRestriction::exact([ProcessorRef::Principal { principal: native.routes[0].logical_executor.clone() }]),
        audiences: ExactRestriction::exact([native.members[0].audience.clone()]),
        delegation_depth: DelegationDepth::remaining(1), ..ExecutionRestrictions::unrestricted()
    };
    let parent = fixture.source.owner.grants().issue_root(&selected.parent.root_authority,
        EvidenceId::new(format!("{name}-report")).unwrap(), native.grant_administrator.clone(),
        native.routes[0].represented_subject.clone(), restrictions.clone()).unwrap();
    selected.helper_targets.push(CompiledAgentSetupTarget {
        target: Some(ConfiguredHelperReportCapability::target()), parent, restrictions,
    });
    selected
}
async fn row(source: &PrivateOriginals, reference: &PrivateOriginalRef) -> SourceRecord {
    let saved = source.store.get(&source.key(reference).unwrap()).await.unwrap().unwrap();
    source.decode_record(&saved, reference).unwrap()
}
fn first_child(source: &PrivateOriginals, reference: &PrivateOriginalRef, session: &meerkat_core::SessionId,
    selected: &CompiledAgentSetupCapability) -> GrantLineageRef
{
    let facts = source.configured_setup(reference).unwrap();
    source.owner.grants().issue_child(&facts.application, &selected.parent,
        source.configured_child_id(reference).unwrap(), facts.route.logical_executor.clone(),
        facts.child_bounds(session)).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_target_recovery_keeps_first_selection_and_missing_selection_refuses() {
    let fixture = Box::pin(ContinuationFixture::for_service_input()).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let submission = captured(&fixture, "configured-target-selection").await;
        let prepared = fixture.source.prepare_local_controller_setup(submission.clone()).await.unwrap();
        let selected = capability(&fixture, "retained-targets");
        fixture.source.retain_configured_setup(&submission.original, prepared.session_id(), selected.clone()).await.unwrap();
        let first = first_child(&fixture.source, &submission.original, prepared.session_id(), &selected);
        let mut changed = selected.clone(); changed.helper_targets.clear();
        assert!(matches!(fixture.source.retain_configured_setup(&submission.original, prepared.session_id(), changed).await,
            Err(PrivateOriginalError::Conflict)));
        let retained = row(&fixture.source, &submission.original).await;
        assert!(retained.configured_setup.as_ref().is_some_and(|saved| saved.capability == selected));
        let basis = fixture.source.configured_setup_basis(&retained, &submission.original, prepared.session_id(), None).await.unwrap().unwrap();
        assert_eq!(super::super::setup_targets::find_lineage(&basis).unwrap().last(), Some(&first));
        assert!(basis.lineage_for(Some(&ConfiguredHelperReportCapability::target())).is_some());
        let admitted = fixture.source.submit_authenticated_original(submission.clone()).await.unwrap();
        assert!(matches!(admitted, PrivateOriginalSubmission::Admitted { turn_ticket: Some(_), .. }));
        fixture.finish(&Ok(admitted)).await.unwrap();
        let binding = fixture.source.binding_evidence(&submission.original).await.unwrap().unwrap();
        let mut missing = row(&fixture.source, &submission.original).await;
        missing.configured_setup = None;
        assert!(matches!(fixture.source.check_configured_setup_binding(&submission.original, &missing, &binding),
            Err(OriginalWriterError::Conflict)));
        assert_eq!(fixture.source.binding_evidence(&submission.original).await.unwrap(), Some(binding.clone()));
        let key = fixture.source.key(&submission.original).unwrap();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        {
            let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
            fixture.source.owner.grants().revoke(&selected.parent.root_authority, &selected.parent, &mut custody).unwrap();
        }
        let claims = binding.candidate();
        let custody = meerkat_runtime::meerkat_machine::AuthenticatedOriginal { original_work: claims.original_work.clone(),
            occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(), original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(), contributing_work: claims.contributing_work.clone(),
            target: claims.target.clone(), authority_basis: claims.authority_basis.clone() };
        let original = &submission.original;
        fixture.source.owner.bind_issued_access_async(binding.clone(), custody, &claims.requester, &claims.ingress_actor,
            Some(&fixture.source.configured_setup(original).unwrap().application), |association| {
                let source = &fixture.source;
                async move { source.persist_or_observe_binding(original, &association, true).await }
            }).await.unwrap();
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
        assert!(fixture.source.owner.grants().resolve_lineage(super::super::setup_targets::find_lineage(&claims.authority_basis).unwrap(),
            &claims.logical_executor, claims.represented_subject.as_ref()).is_err());
        let target = ConfiguredHelperReportCapability::target();
        fixture.source.owner.grants().resolve_lineage(claims.authority_basis.lineage_for(Some(&target)).unwrap(),
            &claims.logical_executor, claims.represented_subject.as_ref()).unwrap().check_current().unwrap();
        let mut changed = binding.candidate().clone();
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &mut changed.authority_basis else { unreachable!() };
        targets.iter_mut().find(|entry| entry.target == target).unwrap().lineage[0] = selected.parent.clone();
        let changed = InputAuthorityAssociation::new(changed).unwrap();
        assert!(fixture.source.persist_or_observe_binding(original, &changed, true).await.is_err());
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
    })).catch_unwind().await;
    let closed = fixture.shutdown().await; assert!(closed.is_ok(), "{closed:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_unbound_child_cannot_acquire_new_targets_and_legacy_scope_remains_exact() {
    let fixture = Box::pin(ContinuationFixture::for_service_input()).await;
    let result = AssertUnwindSafe(Box::pin(async {
        let submission = captured(&fixture, "legacy-before-target-intent").await;
        let prepared = fixture.source.prepare_local_controller_setup(submission.clone()).await.unwrap();
        let selected = capability(&fixture, "legacy-existing-child");
        let first = first_child(&fixture.source, &submission.original, prepared.session_id(), &selected);
        assert!(matches!(fixture.source.retain_configured_setup(&submission.original, prepared.session_id(), selected.clone()).await,
            Err(PrivateOriginalError::Conflict)));
        assert!(row(&fixture.source, &submission.original).await.configured_setup.is_none());
        let report_child = super::super::setup_targets::child_id(&fixture.source.configured_child_id(&submission.original).unwrap(),
            &selected.helper_targets[0].target).unwrap();
        assert!(fixture.source.owner.grants().issued_reference(&report_child).unwrap().is_none());
        let mut legacy = selected; legacy.helper_targets.clear();
        fixture.source.retain_configured_setup(&submission.original, prepared.session_id(), legacy).await.unwrap();
        let record = row(&fixture.source, &submission.original).await;
        let basis = fixture.source.configured_setup_basis(&record, &submission.original, prepared.session_id(), None).await.unwrap().unwrap();
        assert!(matches!(basis, WorkAuthorityBasis::GrantLineage { ref lineage } if lineage.last() == Some(&first)));
        assert!(fixture.source.owner.grants().issued_reference(&report_child).unwrap().is_none());
        assert_eq!(fixture.model_requests(), 0);
    })).catch_unwind().await;
    let closed = fixture.shutdown().await; assert!(closed.is_ok(), "{closed:?}");
    if let Err(payload) = result { std::panic::resume_unwind(payload); }
}

#[path = "selection_writer_tests.rs"]
mod writer_tests;
