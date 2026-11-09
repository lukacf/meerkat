//! Real registered R2 Find, five native helper children, and partial append.
//! The configured parents are issued by the existing trusted fixture producer.
//! No setup-parent approval, local probe witness or replacement grant owner is
//! used as evidence of this Find's native review entry.
use super::*;
use crate::installed_host::{InstalledOperationReviewContext, InstalledOperationReviewPolicy};
use meerkat_authorization_contracts::{
    constraints::ExecutionRestrictions, work_association::WorkAuthorityBasis,
};
use meerkat_core::{
    OperationAuthorizationError, ToolOperationTarget,
    approval::review::{
        BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
        ReviewCandidate, ReviewObservation, ReviewRetirementReason, ReviewVerdict, ReviewerFailure,
    },
    authorization::{AuthorizationOperation, OperationReviewTier},
};
use std::sync::Condvar;

const FIND_CALL: &str = "controller-only-find-and-install";

struct R2Find;
impl InstalledOperationReviewPolicy for R2Find {
    fn review_floor(
        &self,
        context: InstalledOperationReviewContext<'_>,
    ) -> Result<OperationReviewTier, OperationAuthorizationError> {
        let ToolOperationTarget::RegisteredToolBundleOperation {
            bundle, operation, ..
        } = context.target()
        else {
            return Ok(OperationReviewTier::R1);
        };
        if bundle != crate::FIND_AND_INSTALL_BUNDLE || operation != crate::FIND_AND_INSTALL {
            return Ok(OperationReviewTier::R1);
        }
        if context.selected().native_host.as_ref() != Some(context.native()) {
            return Err(OperationAuthorizationError::Unavailable);
        }
        let claims = context.association().candidate();
        let mut members = context.native().members.iter().filter(|member| {
            member.member.member == MEMBER
                && claims.target.logical_owner == member.logical_owner
                && claims.target.context_generation == member.context_generation
                && claims.target.audience == member.audience
        });
        if members.next().is_none() || members.next().is_some() {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(OperationReviewTier::R2)
    }
}
#[derive(Default)]
struct ExactFind {
    callbacks: Mutex<usize>,
    rows: Mutex<Vec<ReviewObservation>>,
    changed: tokio::sync::Notify,
}
#[async_trait]
impl OperationReviewer for ExactFind {
    async fn review(
        &self,
        candidate: &ReviewCandidate<'_>,
    ) -> Result<ReviewVerdict, ReviewerFailure> {
        if candidate.tool_call_id() != FIND_CALL
            || candidate.tool_name() != crate::FIND_AND_INSTALL
            || serde_json::from_str::<serde_json::Value>(candidate.arguments().get()).ok()
                != Some(serde_json::json!({"request":"Find a tool for this original request"}))
        {
            return Err(ReviewerFailure::Unavailable);
        }
        let AuthorizationOperation::Tool(tool) = &candidate.binding().facts().operation else {
            return Err(ReviewerFailure::Unavailable);
        };
        assert_eq!(tool.name, crate::FIND_AND_INSTALL);
        assert_eq!(tool.arguments.get(), candidate.arguments().get());
        *self.callbacks.lock().unwrap() += 1;
        Ok(ReviewVerdict::Allow)
    }
}
impl OperationReviewObserver for ExactFind {
    fn observe(&self, row: ReviewObservation) {
        self.rows.lock().unwrap().push(row);
        self.changed.notify_one();
    }
}
impl ExactFind {
    fn options(self: &Arc<Self>) -> approved_fixture::ReviewedHelperReview {
        approved_fixture::ReviewedHelperReview {
            floor: Arc::new(R2Find),
            review: Arc::new(
                BoundOperationReview::new(self.clone(), meerkat_core::ApprovalService::new(), WAIT)
                    .with_observer(self.clone()),
            ),
        }
    }
    fn used(&self) -> usize {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .filter(|row| row.status == ReviewAttemptStatus::Used)
            .count()
    }
    async fn assert_one_entered_attempt(&self) {
        tokio::time::timeout(WAIT, async {
            loop {
                let changed = self.changed.notified();
                if self.used() != 0 {
                    break;
                }
                changed.await;
            }
        })
        .await
        .expect("actual joined native owners must release reviewed Find");
        let rows = self.rows.lock().unwrap();
        assert!(
            rows.iter()
                .all(|row| row.tool_call_id.as_ref() == FIND_CALL)
        );
        let mut attempts: Vec<Vec<&ReviewObservation>> = vec![];
        for row in rows.iter() {
            if attempts.last().is_none_or(|a| a[0].attempt != row.attempt) {
                assert!(!attempts.iter().any(|a| a[0].attempt == row.attempt));
                attempts.push(vec![]);
            }
            attempts.last_mut().unwrap().push(row);
        }
        assert!((1..=2).contains(&attempts.len()));
        assert_eq!(*self.callbacks.lock().unwrap(), attempts.len());
        use ReviewAttemptStatus::{Allowed, Pending, Retired, Used};
        let final_attempt = attempts.pop().unwrap();
        assert_eq!(
            final_attempt
                .iter()
                .map(|r| (r.status, r.retirement))
                .collect::<Vec<_>>(),
            vec![(Pending, None), (Allowed, None), (Used, None)]
        );
        if let Some(prior) = attempts.pop() {
            let actual = prior
                .iter()
                .map(|r| (r.status, r.retirement))
                .collect::<Vec<_>>();
            assert!(
                actual
                    == vec![
                        (Pending, None),
                        (Retired, Some(ReviewRetirementReason::ContextChanged))
                    ]
                    || actual
                        == vec![
                            (Pending, None),
                            (Allowed, None),
                            (Retired, Some(ReviewRetirementReason::ContextChanged))
                        ]
            );
        }
    }
}

// Called only after the existing actual cleanup, never in a native policy
// callback. Both snapshots release their locks before the diagnostic writer.
fn write_cleanup_diagnostics(fixture: &Fixture, script: &acquisition_denial::Script) {
    use std::io::Write;
    let review = fixture.source.review_observations().drain();
    // Diagnostics must not replace the original assertion if the fixture's
    // observation mutex was poisoned by the already-failed model task.
    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| script.observations()));
    let mut output = std::io::stderr().lock();
    review.write_to(&mut output);
    match observed {
        Ok((feedback, planned, entered)) => {
            let _ = writeln!(
                output,
                "helper_reviewed_roles_observation feedback_count={} planned={} entered={}",
                feedback.len(),
                planned,
                entered
            );
            for item in feedback.iter().take(8) {
                let text = item.text_content();
                let value = (text.len() <= 4096)
                    .then(|| serde_json::from_str::<serde_json::Value>(&text).ok())
                    .flatten();
                let class = match value.as_ref() {
                    Some(value)
                        if value["error"] == "review_unavailable"
                            && value["data"]["kind"] == "context_unavailable" =>
                    {
                        "review_context_unavailable"
                    }
                    Some(value)
                        if value["error"] == "review_unsatisfied"
                            && value["data"]["kind"] == "human_consent_required" =>
                    {
                        "human_consent_required"
                    }
                    Some(value) if value["error"] == "tool_unavailable" => "tool_unavailable",
                    Some(value) if value["error"] == "access_denied" => "access_denied",
                    _ => "unclassified",
                };
                let _ = writeln!(
                    output,
                    "helper_reviewed_roles_feedback is_error={} class={class}",
                    item.is_error
                );
            }
        }
        Err(_) => {
            let _ = writeln!(output, "helper_reviewed_roles_observation unavailable");
        }
    }
}

#[derive(Default)]
struct AfterController {
    state: Mutex<(Option<Vec<GrantLineageRef>>, bool)>,
    resume: Condvar,
    arrived: tokio::sync::Notify,
}
impl AfterController {
    fn observe(&self, lineage: &[GrantLineageRef]) {
        let mut state = self.state.lock().unwrap();
        assert!(
            state.0.replace(lineage.to_vec()).is_none(),
            "one real controller append boundary"
        );
        self.arrived.notify_one();
        let end = std::time::Instant::now() + WAIT;
        while !state.1 {
            let left = end.saturating_duration_since(std::time::Instant::now());
            assert!(
                !left.is_zero(),
                "owner barrier must not strand a blocking worker"
            );
            state = self.resume.wait_timeout(state, left).unwrap().0;
        }
    }
    async fn entered(&self) -> Vec<GrantLineageRef> {
        loop {
            let arrived = self.arrived.notified();
            if let Some(lineage) = &self.state.lock().unwrap().0 {
                return lineage.clone();
            }
            arrived.await;
        }
    }
    fn release(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        self.resume.notify_all();
    }
}

async fn helper_row(fixture: &Fixture) -> toolkit_store::StoredRecord {
    let scope = toolkit_store::StoreScope {
        realm: fixture.scope.realm.clone(),
        owner: format!("toolkit/helper-originals/{}", fixture.scope.owner),
    };
    let rows = fixture.store.list_records(&scope, None, 10).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "one canonical helper source, not a replacement intent"
    );
    fixture.store.get(&rows[0].key).await.unwrap().unwrap()
}
fn intent<'a>(row: &'a toolkit_store::StoredRecord, role: &str) -> &'a serde_json::Value {
    if role == "controller" {
        &row.data.body["controller"]
    } else {
        &row.data.body["controller"][role]
    }
}
fn issued(
    fixture: &Fixture,
    row: &toolkit_store::StoredRecord,
    role: &str,
) -> Option<GrantLineageRef> {
    let id: EvidenceId = serde_json::from_value(intent(row, role)["id"].clone()).unwrap();
    fixture.source.owner.grants().issued_reference(&id).unwrap()
}
fn assert_child(
    fixture: &Fixture,
    row: &toolkit_store::StoredRecord,
    role: &str,
    parent: &[GrantLineageRef],
    lineage: &[GrantLineageRef],
    selection: &InstalledSetupSelection,
) {
    assert_eq!(lineage.len(), parent.len() + 1);
    assert!(lineage.starts_with(parent));
    assert_eq!(issued(fixture, row, role).as_ref(), lineage.last());
    let requested: ExecutionRestrictions =
        serde_json::from_value(intent(row, role)["requested"].clone()).unwrap();
    let parent = fixture
        .source
        .owner
        .grants()
        .resolve_controller_lineage(
            parent,
            &fixture
                ._lifetime
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap()
                .grant_administrator,
            selection.represented_subject.as_ref(),
        )
        .unwrap();
    let expected = parent.restrictions().for_child(&requested).unwrap();
    let child = fixture
        .source
        .owner
        .grants()
        .resolve_controller_lineage(
            lineage,
            &selection.logical_executor,
            selection.represented_subject.as_ref(),
        )
        .unwrap();
    assert_eq!(
        child.restrictions(),
        &expected,
        "{role} keeps its actual parent and exact retained intent"
    );
    parent.check_current().unwrap();
    child.check_current().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_find_issues_all_five_real_helper_roles_from_one_native_entry() {
    let review = Arc::new(ExactFind::default());
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, runtime_store) =
        Fixture::for_reviewed_helper_roles(script.clone(), report.clone(), review.options()).await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, Box::pin(async {
        let first = fixture.submit().await;
        fixture.finish(&first).await.unwrap();
        let (feedback, planned, entered) = script.observations();
        assert_eq!(feedback.len(), 1); assert!(!feedback[0].is_error, "{feedback:?}");
        let reply: serde_json::Value = serde_json::from_str(&feedback[0].text_content()).unwrap();
        assert_eq!(reply["worker"]["state"], "input_admitted");
        assert_eq!(reply["worker"]["source_settlement"], "fresh_acceptance_promoted");
        assert!(planned > 0); assert_eq!(entered, 1);
        let context = report.take_context().await;
        let selection = report.selection.get().unwrap();
        let source = fixture.source.setup_result_source(&context, selection).await.unwrap();
        let row = helper_row(&fixture).await;
        assert_eq!(row.data.body["state"]["phase"], "bound");
        assert_eq!(row.data.body["state"]["association"], serde_json::to_value(&source.helper).unwrap());
        let basis = &source.helper.candidate().authority_basis;
        let WorkAuthorityBasis::TargetedGrantLineages { base: Some(_), targets } = basis
            else { panic!("actual catalog and independent target children required") };
        assert_eq!(targets.len(), 3);
        let capability = selection.report_capability.as_ref().unwrap();
        let rows = [
            ("controller", selection.controller_parent.as_slice(), source.helper.candidate().controller_grant_lineage.as_slice()),
            ("report", capability.parent.as_slice(), basis.lineage_for(Some(&crate::installed_host::acquisition::ConfiguredHelperReportCapability::target())).unwrap()),
            ("account", capability.account_parent.as_ref().unwrap().as_slice(), basis.lineage_for(Some(&crate::installed_host::acquisition::ConfiguredHelperReportCapability::account_target())).unwrap()),
            ("public", capability.public_parent.as_ref().unwrap().as_slice(), basis.lineage_for(Some(&crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target())).unwrap()),
            ("catalog", selection.catalog_capability.as_ref().unwrap().parent.as_slice(), basis.lineage_for(None).unwrap()),
        ];
        let mut child_ids = std::collections::BTreeSet::new();
        for (role, parent, lineage) in rows {
            assert_child(&fixture, &row, role, parent, lineage, selection);
            assert!(child_ids.insert(lineage.last().unwrap().grant_id.clone()));
        }
        assert_eq!(child_ids.len(), 5);
        let helper_session = context.origin_session_id().unwrap().clone();
        let original = fixture.binding().await.unwrap().unwrap();
        assert_eq!(source.contributors, vec![original]);
        let helper = AgentIdentity::parse(crate::installed_host::controller_first_input_fixture::HELPER).unwrap();
        let cursor = fixture.runtime.completion_cursor(&helper).await;
        drop(context);
        report.release();
        fixture.runtime.wait_for_completion(&helper, cursor, WAIT).await.unwrap();
        acquisition_approved::consumed_native_input(runtime_store.as_ref(), &helper_session,
            &source.helper, None, tokio::time::Instant::now() + WAIT).await;
        assert!(fixture.store.get(&row.key).await.unwrap().unwrap() == row,
            "report completion must not rewrite the issued helper source or any intent");
    }))).catch_unwind().await;
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    write_cleanup_diagnostics(&fixture, &script);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("reviewed helper roles deadline"),
        Err(p) => std::panic::resume_unwind(p),
    }
    review.assert_one_entered_attempt().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reviewed_find_keeps_first_native_append_when_actual_report_parent_is_revoked() {
    let review = Arc::new(ExactFind::default());
    let script = acquisition_denial::Script::for_approved();
    let report = Report::new(script.tools().result_tool());
    let (fixture, runtime_store) =
        Fixture::for_reviewed_helper_roles(script.clone(), report.clone(), review.options()).await;
    let barrier = Arc::new(AfterController::default());
    let observed = barrier.clone();
    assert!(
        fixture
            .source
            .helper_controller_appended
            .set(Arc::new(move |lineage| observed.observe(lineage)))
            .is_ok()
    );
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT, Box::pin(async {
        let first = fixture.submit().await;
        let controller = barrier.entered().await;
        assert_eq!(review.used(), 0, "actual entered append owner has not ended");
        let before = helper_row(&fixture).await;
        assert_eq!(before.data.body["state"]["phase"], "controller_pending");
        let selection = report.selection.get().unwrap();
        assert_child(&fixture, &before, "controller", &selection.controller_parent, &controller, selection);
        for role in ["report", "account", "public", "catalog"] { assert!(issued(&fixture, &before, role).is_none()); }
        let parent = selection.report_capability.as_ref().unwrap().parent.last().unwrap();
        // The native append has returned and released its own reservation.
        // Revoke only the actual independently selected report parent, using
        // the same machine's controller mutation owner. No result is injected.
        {
            let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
            fixture.source.owner.grants().revoke(&meerkat_core::AdmissionGuard::none(), &parent.root_authority, parent, &mut custody).unwrap();
        }
        let resolved = fixture.source.owner.grants().resolve_controller_lineage(
            &selection.report_capability.as_ref().unwrap().parent,
            &fixture._lifetime.selected_policy().native_host.as_ref().unwrap().grant_administrator,
            selection.represented_subject.as_ref());
        assert!(resolved.is_err(), "the exact native report parent is now revoked");
        barrier.release();
        fixture.finish(&first).await.unwrap();
        let (feedback, planned, entered) = script.observations();
        assert_eq!(feedback.len(), 1); assert!(feedback[0].is_error, "{feedback:?}");
        assert!(planned > 0); assert_eq!(entered, 1);
        assert!(report.leaf.context.lock().unwrap().is_none());
        assert!(report.feedback.lock().unwrap().is_empty());
        assert!(fixture.source.helper_sources.lock().unwrap().is_empty());
        let after = helper_row(&fixture).await;
        assert!(after == before, "the first append and later refusal retain the exact original pending source intent");
        // The actual completion owner copied these coordinates directly from
        // NativeWorkPreparation into this canonical helper source. Reopen that
        // exact existing effect, rather than synthesizing an effect name or
        // treating failed tool feedback as an effect-settlement receipt.
        let operation: crate::original_ingress::helper_originals::HelperOperation =
            serde_json::from_value(before.data.body["facts"]["operation"].clone()).unwrap();
        let effect_key = toolkit_store::EffectKey {
            record: operation.acquisition.clone(), id: operation.effect.clone(),
        };
        let effect = fixture.store.get_effect(&effect_key).await.unwrap().unwrap();
        assert_eq!(effect.key, effect_key);
        assert_eq!(effect.record_revision, 1);
        assert_eq!(effect.request_digest, effect.intent.digest().unwrap());
        assert_eq!(effect.attempt, 1, "one actual claimed completion attempt");
        assert!(effect.lease.is_none(), "the failed native completion returned and acknowledged its uncertainty");
        assert_eq!(effect.status, toolkit_store::EffectStatus::Uncertain);
        let Some(toolkit_store::EffectOutcome::Uncertain { evidence }) = &effect.outcome
            else { panic!("partial native append must retain uncertainty, never a success receipt") };
        assert_eq!(evidence.schema, "meerkat_toolkit.native_helper_observation");
        assert_eq!(evidence.version, 1);
        assert_eq!(evidence.body["kind"], "completion_uncertain");
        let admission: meerkat_workgraph::WorkAdmission =
            serde_json::from_value(before.data.body["facts"]["admission"].clone()).unwrap();
        assert_eq!(evidence.body["admission_key"], serde_json::to_value(&admission.key).unwrap());
        // observed_admission describes the owner's earlier observation. It is
        // not inferred from the controller append and need not be null.
        let events = fixture.store.read_events(&effect_key.record, 0, 64).await.unwrap();
        assert!(events.len() < 64, "the finite first-attempt journal must be complete");
        let effect_events: Vec<_> = events.iter().filter(|event|
            event.data.schema == "toolkit.store.effect-event"
                && event.data.body["effect_id"] == effect_key.id).collect();
        assert_eq!(effect_events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(),
            vec!["toolkit.store.effect.claimed", "toolkit.store.effect.acknowledged"],
            "no Applied, NotApplied or RetrySafe reconciliation may replace the actual partial outcome");
        let acknowledged = effect_events.last().unwrap();
        assert_eq!(acknowledged.data.version, 1);
        let retained: toolkit_store::EffectOutcome =
            serde_json::from_value(acknowledged.data.body["evidence"].clone()).unwrap();
        assert_eq!(Some(retained), effect.outcome, "the journal retains the same uncertainty, with no success receipt");
        assert_child(&fixture, &after, "controller", &selection.controller_parent, &controller, selection);
        for role in ["report", "account", "public", "catalog"] { assert!(issued(&fixture, &after, role).is_none()); }
        let helper = MemberIdentity::from(crate::installed_host::controller_first_input_fixture::HELPER);
        if let Some(member) = fixture.native.mob_handle().get_member(&helper).await.unwrap()
            && let Some(session) = member.bridge_session_id()
        {
            assert!(acquisition_approved::native_history(runtime_store.as_ref(), session).await.is_empty());
        }
        assert_eq!(fixture.store.list_records(&acquisition_approved::scope(&fixture), None, 10).await.unwrap().len(), 1);
        let replay = fixture.submit().await;
        fixture.finish(&replay).await.unwrap();
        assert!(helper_row(&fixture).await == before);
        assert!(fixture.store.get_effect(&effect_key).await.unwrap().as_ref() == Some(&effect),
            "original input replay must preserve the actual uncertain completion record");
        assert_eq!(script.observations().2, 1, "original input replay must not create a replacement Find or intent");
        assert_eq!(issued(&fixture, &before, "controller").as_ref(), controller.last());
    }))).catch_unwind().await;
    barrier.release();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    write_cleanup_diagnostics(&fixture, &script);
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("partial native helper append deadline"),
        Err(p) => std::panic::resume_unwind(p),
    }
    review.assert_one_entered_attempt().await;
}
