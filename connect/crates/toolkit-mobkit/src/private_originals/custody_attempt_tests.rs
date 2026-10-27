//! Canonical schema/CAS controls with real native binding and SQLite rows.
//! Schema and foreground controls execute no native admission, model, recovery task or wake.
//! Explicit source-only staging controls label their synthetic settlement separately.
use super::*;
use crate::original_ingress::custody_attempt::CustodyChange;
use crate::original_ingress::tests::{claims, initialized, principal, private_event, private_originals_with_store, source};
use meerkat_authorization_contracts::{constraints::ExecutionRestrictions, work_association::InputAuthorityAssociation};
use meerkat_core::{AgentError, AgentLlmClient, ControllerModelClient, ControllerModelSelection,
    LlmStreamResult, Message, Provider, ProviderParamsOverride, SessionId, SessionLlmIdentity, ToolDef};
use meerkat_runtime::{Input, LogicalRuntimeId, PromptInput};
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::Ordering;
use meerkat_runtime::meerkat_machine::AuthenticatedOriginalSource;
#[path = "foreground_test_store.rs"]
mod foreground_test_store;
use foreground_test_store::ForegroundTestStore;

const CONTENT: &str = "one exact prepared source input";
fn id(value: &str) -> EvidenceId { EvidenceId::new(value).unwrap() }
fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(SessionLlmIdentity {
        model: "custody-schema-fixture".into(), provider: Provider::Anthropic,
        self_hosted_server_id: None, provider_params: None, auth_binding: None,
    }, serde_json::from_value(serde_json::json!({"realm":"test-realm","account":"custody-schema-fixture"})).unwrap(),
        "custody-schema-fixture".into(), "custody-schema-fixture".into())
}
struct UnusedClient;
#[async_trait::async_trait]
impl AgentLlmClient for UnusedClient {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> { Some(selection()) }
    async fn stream_response(&self, _: &[Message], _: &[Arc<ToolDef>], _: u32, _: Option<f32>,
        _: Option<&ProviderParamsOverride>) -> Result<LlmStreamResult, AgentError> {
        panic!("schema controls must not execute a model")
    }
    fn provider(&self) -> Provider { Provider::Anthropic }
    fn model(&self) -> &str { "custody-schema-fixture" }
}
struct Fixture {
    adapter: Arc<PrivateOriginals>, original: Arc<RetainedToolkitOriginal>,
    reference: PrivateOriginalRef, session: SessionId, input: Input,
    unbound: StoredRecord, _directory: tempfile::TempDir,
    faults: Arc<ForegroundTestStore>,
    prepared: Option<crate::original_ingress::PreparedToolkitOriginal>,
}
impl Fixture {
    async fn new() -> Self { Self::with_binding_rebase(false).await }
    async fn with_binding_rebase(rebase: bool) -> Self {
        let directory = tempfile::Builder::new().permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir().unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let faults = Arc::new(ForegroundTestStore::new(Arc::new(
            toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap())));
        let adapter = private_originals_with_store(owner.clone(), faults.clone()).await;
        let base = owner.grants().issue_root(&principal("admin"), id("base"), principal("admin"),
            None, ExecutionRestrictions::unrestricted()).unwrap();
        let mut event = private_event("schema"); event.content_sha256 = digest(CONTENT.as_bytes());
        let reference = adapter.retain_event(event).await.unwrap();
        let unbound = adapter.store.get(&adapter.key(&reference).unwrap()).await.unwrap().unwrap();
        let session = SessionId::new();
        let mut candidate = claims("schema", &base).candidate().clone();
        candidate.target.logical_runtime = id(&LogicalRuntimeId::for_session(&session).to_string());
        candidate.target.context = id(&session.to_string());
        candidate.controller_model = Some(selection()); candidate.controller_grant_lineage = vec![base];
        let association = InputAuthorityAssociation::new(candidate).unwrap();
        let mut requested = ExecutionRestrictions::unrestricted();
        requested.delegation_depth = meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0);
        let original = owner.enroll_new_authenticated_async(association.clone(), source(&association),
            &principal("user"), &principal("frontend"), &principal("admin"), id("schema-child"),
            requested, |association| {
                let adapter = adapter.clone(); let reference = reference.clone(); let faults = faults.clone();
                async move {
                    if rebase {
                        let key = adapter.key(&reference).unwrap();
                        let before = adapter.store.get(&key).await.unwrap().unwrap();
                        faults.arm_association_no_commit();
                        assert!(matches!(adapter.persist_binding(&reference, &association).await,
                            Err(OriginalWriterError::Unavailable)));
                        assert!(adapter.store.get(&key).await.unwrap().unwrap() == before);
                        assert_eq!(faults.association_refusals.load(Ordering::SeqCst), 1);
                        // Changed immutable source content cannot borrow the entered-write witness.
                        let mut changed = adapter.decode_record(&before, &reference).unwrap();
                        changed.event.content_sha256 = digest(b"different authenticated source content");
                        assert!(adapter.owner.retain_first_binding_write(adapter.store.as_ref(),
                            &adapter.scope, &key, Arc::new(rewritten(&before, encode(changed).unwrap())), &association).is_err());
                        // A current writer may advance revision without changing any source facts.
                        let advanced = adapter.store.compare_and_swap(Transition {
                            key, expected_revision: before.revision, expected_digest: Some(before.content_digest.clone()),
                            claim: None, data: before.data.clone(), events: vec![], effects: vec![],
                        }).await.unwrap();
                        assert_eq!(advanced.revision, before.revision + 1);
                        assert_eq!(advanced.data, before.data);
                    }
                    adapter.persist_binding(&reference, &association).await
                }
            }).await.unwrap();
        adapter.hold_binding(reference.clone(), original.clone()).await.unwrap();
        let row = adapter.store.get(&adapter.key(&reference).unwrap()).await.unwrap().unwrap();
        let record = adapter.decode_record(&row, &reference).unwrap();
        let prepared = owner.prepare_authenticated_input(&original, &session,
            Input::Prompt(PromptInput::new(CONTENT, None)),
            cold_service_admission::source_event_key(&record).unwrap(),
            original.association().candidate().original_authentication.clone(),
            ControllerModelClient::new(selection(), Arc::new(UnusedClient))).unwrap();
        let input = prepared.input().clone();
        Self { adapter, original, reference, session, input, unbound, _directory: directory,
            faults, prepared: Some(prepared) }
    }
    async fn row(&self) -> StoredRecord {
        self.adapter.store.get(&self.adapter.key(&self.reference).unwrap()).await.unwrap().unwrap()
    }
    fn held(&self) -> Arc<HeldPrivateOriginal> {
        self.adapter.retained.lock().unwrap().iter().find(|held| held.reference == self.reference).unwrap().clone()
    }
    fn assert_no_candidate(&self) {
        let claims = self.original.association().candidate();
        let ingress = self.input.header().ingress_context.as_ref().unwrap();
        assert!(self.adapter.owner.authenticated_original(ingress, &claims.ingress_namespace,
            &claims.original_work).unwrap().is_none());
        assert!(self.adapter.owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work).unwrap().is_none());
    }
    fn enrollment(&self, row: &StoredRecord) -> Transition {
        self.adapter.prepare_custody_enrollment(row, &self.reference, &self.original,
            &self.session, &self.input).unwrap()
    }
}
fn without_attempt(data: &RecordData) -> serde_json::Value {
    let mut value = data.body.clone(); value.as_object_mut().unwrap().remove("custody_attempt"); value
}
fn rewritten(row: &StoredRecord, data: RecordData) -> StoredRecord {
    let mut row = row.clone(); row.content_digest = data.digest().unwrap(); row.data = data; row
}

#[tokio::test]
async fn canonical_custody_cas_preserves_rows_and_refuses_stale_or_reset_attempts() {
    let f = Fixture::new().await;
    let before = f.row().await;
    assert_eq!(before.data.version, SOURCE_VERSION);
    assert!(before.data.body.get("custody_attempt").is_none());
    let initial = f.enrollment(&before);
    assert!(initial.events.is_empty() && initial.effects.is_empty());
    assert_eq!(initial.expected_revision, before.revision);
    assert_eq!(initial.expected_digest.as_ref(), Some(&before.content_digest));
    assert!(f.row().await == before, "preparing a CAS must not write the row");
    let enrolled = f.adapter.store.compare_and_swap(initial.clone()).await.unwrap();
    assert_eq!(enrolled.data.version, CUSTODY_SOURCE_VERSION);
    assert_eq!(without_attempt(&enrolled.data), before.data.body);
    assert!(same_source_origin(&before, &enrolled));
    assert!(f.adapter.store.compare_and_swap(initial).await.is_err());
    let attempt = f.adapter.decode_record(&enrolled, &f.reference).unwrap().custody_attempt.unwrap();
    assert!(attempt.matches_input(&f.input).unwrap());
    let charge = f.adapter.prepare_custody_transition(&enrolled, &f.reference,
        attempt.changed(CustodyChange::ChargeResolution).unwrap()).unwrap();

    // A legitimate unrelated cancellation wins first. The stale custody CAS
    // must fail; preparing from the reread preserves that cancellation.
    let mut cancelled = f.adapter.decode_record(&enrolled, &f.reference).unwrap();
    cancelled.operation_cancelled = true;
    let changed = f.adapter.store.compare_and_swap(Transition {
        key: enrolled.key.clone(), expected_revision: enrolled.revision,
        expected_digest: Some(enrolled.content_digest.clone()), claim: None,
        data: encode(cancelled).unwrap(), events: vec![], effects: vec![],
    }).await.unwrap();
    assert_eq!(changed.data.version, CUSTODY_SOURCE_VERSION);
    assert!(f.adapter.store.compare_and_swap(charge).await.is_err());
    let charged = f.adapter.store.compare_and_swap(f.adapter.prepare_custody_transition(
        &changed, &f.reference, attempt.changed(CustodyChange::ChargeResolution).unwrap()).unwrap()).await.unwrap();
    assert_eq!(without_attempt(&charged.data), without_attempt(&changed.data));
    let current = f.adapter.decode_record(&charged, &f.reference).unwrap();
    assert!(current.operation_cancelled);
    let terminal = current.custody_attempt.unwrap().changed(CustodyChange::Withdraw).unwrap();
    let withdrawn = f.adapter.store.compare_and_swap(f.adapter.prepare_custody_transition(
        &charged, &f.reference, terminal.clone()).unwrap()).await.unwrap();
    let reopened = decode_source_record(&serde_json::from_value(serde_json::to_value(&withdrawn).unwrap()).unwrap()).unwrap();
    assert!(reopened.custody_attempt.as_ref() == Some(&terminal));
    assert!(f.adapter.prepare_custody_transition(&withdrawn, &f.reference, attempt).is_err());
    assert!(f.adapter.prepare_custody_enrollment(&withdrawn, &f.reference, &f.original,
        &f.session, &f.input).is_err());
}

#[tokio::test]
async fn custody_enrollment_requires_canonical_binding_and_exact_prepared_input() {
    let f = Fixture::new().await;
    let row = f.row().await;
    assert!(f.adapter.prepare_custody_enrollment(&f.unbound, &f.reference, &f.original,
        &f.session, &f.input).is_err());
    assert!(f.adapter.prepare_custody_enrollment(&row, &f.reference, &f.original,
        &SessionId::new(), &f.input).is_err());
    assert!(f.adapter.prepare_custody_enrollment(&row, &f.reference, &f.original,
        &f.session, &Input::Prompt(PromptInput::new(CONTENT, None))).is_err());
    let Input::Prompt(mut changed) = f.input.clone() else { unreachable!() };
    changed.content = meerkat_core::ContentInput::Text("changed content".into());
    assert!(f.adapter.prepare_custody_enrollment(&row, &f.reference, &f.original,
        &f.session, &Input::Prompt(changed)).is_err());
    let Input::Prompt(mut changed) = f.input.clone() else { unreachable!() };
    changed.header.idempotency_key = Some(meerkat_runtime::identifiers::IdempotencyKey::new("other"));
    assert!(f.adapter.prepare_custody_enrollment(&row, &f.reference, &f.original,
        &f.session, &Input::Prompt(changed)).is_err());
    let enrolled = f.adapter.store.compare_and_swap(f.enrollment(&row)).await.unwrap();
    let attempt = decode_source_record(&enrolled).unwrap().custody_attempt.unwrap();
    let Input::Prompt(mut changed) = f.input.clone() else { unreachable!() };
    changed.header.id = meerkat_core::lifecycle::InputId::new();
    let changed = Input::Prompt(changed);
    assert!(!attempt.matches_input(&changed).unwrap());
    assert!(f.adapter.prepare_custody_enrollment(&enrolled, &f.reference, &f.original,
        &f.session, &changed).is_err());

    // Existing retained Input correlation uses exact ID/bytes, independently
    // of the older native replay-equivalence helper that normalizes ID/time.
    let mut record = decode_source_record(&enrolled).unwrap();
    record.bound_service_input = Some(serde_json::from_value(serde_json::to_value(&f.input).unwrap()).unwrap());
    assert!(validate_custody_record(&record).is_ok());
    record.bound_service_input = Some(serde_json::from_value(serde_json::to_value(changed).unwrap()).unwrap());
    assert!(validate_custody_record(&record).is_err());
}

#[tokio::test]
async fn source_schema_versions_and_immutable_purpose_remain_exact() {
    let f = Fixture::new().await;
    let before = f.row().await;
    assert_eq!(encode(decode_source_record(&before).unwrap()).unwrap(), before.data);
    assert!(before.data.body.get("shared_preparations").is_none());
    assert!(before.data.body.get("planning_source_evidence").is_none());
    let canonical = canonical_json(&before.data.body).unwrap();
    let decoded = decode_source_record(&before).unwrap();
    assert_eq!(canonical_json(&decoded).unwrap(), canonical);
    assert_eq!(encode(decoded).unwrap().digest().unwrap(), before.content_digest);
    // Shared development records are outside this release's source contract.
    // Unknown fields must refuse, never disappear during canonicalization.
    for shared in [serde_json::json!([{"source":"shared-development-row"}]), serde_json::json!([])] {
        let mut data = before.data.clone();
        data.body["shared_preparations"] = shared;
        let injected = rewritten(&before, data);
        assert!(decode_source_record(&injected).is_err());
    }
    let data = f.enrollment(&before).data;
    let enrolled = rewritten(&before, data.clone());
    assert!(decode_source_record(&enrolled).is_ok());
    assert_eq!(encode(decode_source_record(&enrolled).unwrap()).unwrap(), data);
    for (version, remove) in [(SOURCE_VERSION, false), (CUSTODY_SOURCE_VERSION, true), (5, false)] {
        let mut data = enrolled.data.clone(); data.version = version;
        if remove { data.body.as_object_mut().unwrap().remove("custody_attempt"); }
        assert!(decode_source_record(&rewritten(&enrolled, data)).is_err());
    }
    for field in ["session_id", "prepared_input_sha256"] {
        let mut data = enrolled.data.clone();
        data.body["custody_attempt"][field] = if field == "session_id" {
            serde_json::to_value(SessionId::new()).unwrap()
        } else { serde_json::json!("not-a-digest") };
        assert!(decode_source_record(&rewritten(&enrolled, data)).is_err());
    }
    let mut data = enrolled.data.clone(); data.body["final_association"] = serde_json::Value::Null;
    assert!(decode_source_record(&rewritten(&enrolled, data)).is_err());
    let mut changed = decode_source_record(&f.unbound).unwrap();
    changed.event.content_sha256 = digest(b"different authenticated source content");
    let changed = rewritten(&f.unbound, encode(changed).unwrap());
    assert!(decode_source_record(&changed).is_ok());
    assert!(!same_source_origin(&f.unbound, &changed));
}


#[tokio::test]
async fn first_binding_failed_write_rebases_same_origin_and_keeps_prewrite_witness() {
    let f = Fixture::with_binding_rebase(true).await;
    let row = f.row().await;
    assert!(f.original.first_binding_for(&row.key));
    let key = f.adapter.key(&f.reference).unwrap();
    let mut wrong_key = row.clone();
    wrong_key.key.id.push_str("/other-original");
    assert!(matches!(f.adapter.owner.retain_first_binding_write(f.adapter.store.as_ref(),
        &f.adapter.scope, &key, Arc::new(wrong_key), f.original.association()),
        Err(OriginalWriterError::WrongOwner)));
    assert!(f.row().await == row);
    assert!(f.adapter.binding_evidence(&f.reference).await.unwrap().as_ref() == Some(f.original.association()));
    f.assert_no_candidate();
    f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &f.input).await.unwrap();
    assert!(f.adapter.decode_record(&f.row().await, &f.reference).unwrap()
        .custody_attempt.unwrap().matches_input(&f.input).unwrap());
    f.assert_no_candidate();
}

#[tokio::test]
async fn first_attempt_lost_reply_reconciles_exact_input_before_candidate() {
    let mut f = Fixture::new().await;
    f.faults.arm_attempt_lost_reply();
    assert!(matches!(f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &f.input).await,
        Err(PrivateOriginalError::Unavailable)));
    let committed = f.row().await;
    assert_eq!(f.faults.attempt_commits.load(Ordering::SeqCst), 1);
    assert!(decode_source_record(&committed).unwrap().custody_attempt.unwrap().matches_input(&f.input).unwrap());
    f.assert_no_candidate();
    f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &f.input).await.unwrap();
    assert!(f.row().await == committed, "exact reconciliation must not reset or rewrite the attempt");
    let Input::Prompt(mut changed) = f.input.clone() else { unreachable!() };
    changed.header.id = meerkat_core::lifecycle::InputId::new();
    assert!(f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &Input::Prompt(changed)).await.is_err());
    assert!(f.row().await == committed);
    let (_, pending) = f.prepared.take().unwrap().stage().unwrap();
    let claims = f.original.association().candidate();
    assert!(f.adapter.owner.authenticated_original(f.input.header().ingress_context.as_ref().unwrap(),
        &claims.ingress_namespace, &claims.original_work).unwrap().is_some());
    drop(pending); // Source staging only; no synthetic native acceptance claim.
    f.assert_no_candidate();
    assert!(f.row().await == committed);
}

#[tokio::test]
async fn exact_first_input_rebind_preserves_identity_and_refuses_changed_semantics() {
    let f = Fixture::new().await;
    let held = f.held();
    assert!(cold_service_admission::RetainedBoundInput::retain_for_attempt(&held.first_input, &f.input).unwrap().is_none());
    let row = f.row().await;
    let record = decode_source_record(&row).unwrap();
    let candidate = f.adapter.owner.prepare_authenticated_input(&f.original, &f.session,
        Input::Prompt(PromptInput::new(CONTENT, None)), cold_service_admission::source_event_key(&record).unwrap(),
        f.original.association().candidate().original_authentication.clone(),
        ControllerModelClient::new(selection(), Arc::new(UnusedClient))).unwrap();
    assert_ne!(candidate.input().id(), f.input.id());
    let saved = cold_service_admission::RetainedBoundInput::retain_for_attempt(&held.first_input, candidate.input()).unwrap().unwrap();
    assert!(saved.header().ingress_context.is_none(), "saved bytes are not process authentication");
    let rebound = f.adapter.owner.prepare_authenticated_input(&f.original, &f.session, saved,
        cold_service_admission::source_event_key(&record).unwrap(),
        f.original.association().candidate().original_authentication.clone(),
        ControllerModelClient::new(selection(), Arc::new(UnusedClient))).unwrap();
    assert_eq!(serde_json::to_value(rebound.input()).unwrap(), serde_json::to_value(&f.input).unwrap());
    assert!(rebound.input().header().ingress_context.is_some());
    for change_association in [false, true] {
        let Input::Prompt(mut changed) = candidate.input().clone() else { unreachable!() };
        if change_association { changed.header.authority_association = None; }
        else { changed.content = meerkat_core::ContentInput::Text("changed source payload".into()); }
        assert!(cold_service_admission::RetainedBoundInput::retain_for_attempt(&held.first_input, &Input::Prompt(changed)).is_err());
    }
    f.adapter.persist_first_custody_attempt(&held, &f.session, rebound.input()).await.unwrap();
    f.assert_no_candidate();
}

#[tokio::test]
async fn first_attempt_persistence_does_not_override_source_cancellation_before_staging() {
    let mut f = Fixture::new().await;
    f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &f.input).await.unwrap();
    f.adapter.record_operation_cancelled(&f.reference).await.unwrap();
    assert!(f.prepared.take().unwrap().stage().is_err());
    let row = f.row().await;
    assert!(decode_source_record(&row).unwrap().operation_cancelled);
    assert!(f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &f.input).await.is_err());
    assert!(f.row().await == row);
}

#[tokio::test]
async fn legacy_bound_row_does_not_recreate_first_write_provenance_on_owner_restore() {
    let f = Fixture::new().await;
    let association = f.original.association().clone();
    let domain = f.adapter.owner.ingress_domain().clone();
    let Fixture { adapter, original, reference, session, input, unbound, _directory: directory,
        faults, prepared } = f;
    drop((adapter, original, input, unbound, faults, prepared));
    let owner = Arc::new(ToolkitOriginalOwner::restore(
        Arc::new(meerkat_authorization::grants::FileGrantOwnerStore::new(directory.path().join("grants"))),
        Arc::new(meerkat_authorization::clock::HostAuthorizationClock), domain).unwrap());
    let adapter = private_originals_with_store(owner.clone(), Arc::new(
        toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap())).await;
    let original = owner.bind_issued_access_async(association.clone(), source(&association),
        &principal("user"), &principal("frontend"), Some(&principal("admin")), |association| {
            let adapter = adapter.clone(); let reference = reference.clone();
            async move { adapter.persist_or_observe_binding(&reference, &association, true).await }
        }).await.unwrap();
    adapter.hold_binding(reference.clone(), original.clone()).await.unwrap();
    let row = adapter.store.get(&adapter.key(&reference).unwrap()).await.unwrap().unwrap();
    assert!(!original.first_binding_for(&row.key));
    let record = decode_source_record(&row).unwrap();
    let prepared = owner.prepare_authenticated_input(&original, &session,
        Input::Prompt(PromptInput::new(CONTENT, None)), cold_service_admission::source_event_key(&record).unwrap(),
        association.candidate().original_authentication.clone(),
        ControllerModelClient::new(selection(), Arc::new(UnusedClient))).unwrap();
    let held = adapter.retained.lock().unwrap().iter().find(|held| held.reference == reference).unwrap().clone();
    assert!(matches!(adapter.persist_first_custody_attempt(&held, &session, prepared.input()).await,
        Err(PrivateOriginalError::Unavailable)));
    assert!(adapter.store.get(&row.key).await.unwrap().unwrap() == row);
    let claims = association.candidate();
    assert!(owner.authenticated_original(prepared.input().header().ingress_context.as_ref().unwrap(),
        &claims.ingress_namespace, &claims.original_work).unwrap().is_none());
}


#[tokio::test]
async fn admitted_only_observation_cannot_restage_after_actual_source_release() {
    let mut f = Fixture::new().await;
    let (_, staged) = f.prepared.take().unwrap().stage().unwrap();
    // This is the existing source-settlement primitive, not a claim that this
    // fixture entered the native input runtime or completed a model turn.
    staged.admit().unwrap();
    let record = decode_source_record(&f.row().await).unwrap();
    let prepared = f.adapter.owner.prepare_authenticated_input(&f.original, &f.session,
        Input::Prompt(PromptInput::new(CONTENT, None)), cold_service_admission::source_event_key(&record).unwrap(),
        f.original.association().candidate().original_authentication.clone(),
        ControllerModelClient::new(selection(), Arc::new(UnusedClient))).unwrap();
    let (_, replay_guard) = prepared.admitted().unwrap().unwrap();
    f.adapter.owner.release_original(&f.original).unwrap();
    drop(prepared);
    replay_guard.admit().unwrap();
    f.assert_no_candidate();
    assert!(decode_source_record(&f.row().await).unwrap().custody_attempt.is_none());
}

#[tokio::test]
async fn source_guard_acknowledgement_survives_later_callback_error_and_panic() {
    use super::governed_submit::SourcePublication;
    use std::cell::Cell;
    use std::panic::{AssertUnwindSafe, catch_unwind};

    for panic_after_acknowledgement in [false, true] {
        let mut f = Fixture::new().await;
        f.adapter.persist_first_custody_attempt(&f.held(), &f.session, &f.input)
            .await.unwrap();
        let canonical = f.row().await;
        let (_, guard) = f.prepared.take().unwrap().stage().unwrap();
        assert!(guard.is_first_candidate());
        let publication = Mutex::new(SourcePublication::Pending(guard));
        let callbacks = Cell::new(0);

        // This consumes the actual source guard only. The callback is a
        // controlled publication failure, not native admission or a model turn.
        let observed = catch_unwind(AssertUnwindSafe(|| {
            publication.lock().unwrap().admit_then(|| {
                callbacks.set(callbacks.get() + 1);
                if panic_after_acknowledgement {
                    panic!("controlled failure after source acknowledgement");
                }
                Err("controlled later publication refusal".to_owned())
            })
        }));
        if panic_after_acknowledgement {
            assert!(observed.is_err());
        } else {
            assert_eq!(observed.unwrap(), Err("controlled later publication refusal".to_owned()));
        }
        assert_eq!(publication.is_poisoned(), panic_after_acknowledgement);
        assert_eq!(callbacks.get(), 1);
        {
            // Production also recovers this same poisoned owner lock. Its
            // actual source acknowledgement must survive the outer panic.
            let mut state = publication.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let retained_result = match &*state {
                SourcePublication::Settled { source_acknowledged: true, result } => {
                    assert!(result.is_err());
                    result.clone()
                }
                _ => panic!("the actual source acknowledgement was lost"),
            };
            let repeat = state.admit_then(|| {
                callbacks.set(callbacks.get() + 1);
                Ok(())
            });
            assert_eq!(repeat, retained_result);
            assert_eq!(callbacks.get(), 1, "settlement must not invoke the callback twice");
            assert!(matches!(&*state, SourcePublication::Settled {
                source_acknowledged: true, result: Err(_),
            }));
        }

        // The successful source primitive remains admitted even though the
        // later callback failed. A new native-bound preparation gets only an
        // admitted guard, never a second first-Candidate guard.
        let record = decode_source_record(&canonical).unwrap();
        let rebound = f.adapter.owner.prepare_authenticated_input(&f.original, &f.session,
            Input::Prompt(PromptInput::new(CONTENT, None)),
            cold_service_admission::source_event_key(&record).unwrap(),
            f.original.association().candidate().original_authentication.clone(),
            ControllerModelClient::new(selection(), Arc::new(UnusedClient))).unwrap();
        let (_, admitted_only) = rebound.admitted().unwrap().unwrap();
        assert!(!admitted_only.is_first_candidate());
        drop(admitted_only);
        drop(rebound);
        assert!(f.row().await == canonical,
            "source guard settlement must not invent a durable terminal write");
    }
}

/// Component control over actual SQLite and grant-owner reopen. It selects the
/// existing cold census explicitly; no native admission or installed opener is
/// claimed by this schema fixture. The native delivery control is separate.
#[tokio::test]
async fn withdrawn_custody_reopens_as_terminal_absence_without_reenrollment() {
    let f = Fixture::new().await;
    let bound = f.row().await;
    let enrolled = f.adapter.store.compare_and_swap(f.enrollment(&bound)).await.unwrap();
    let attempt = f.adapter.decode_record(&enrolled, &f.reference).unwrap().custody_attempt.unwrap();
    let withdrawn = f.adapter.store.compare_and_swap(f.adapter.prepare_custody_transition(
        &enrolled, &f.reference, attempt.changed(CustodyChange::Withdraw).unwrap()).unwrap()).await.unwrap();
    let association = f.original.association().clone();
    let reference = f.reference.clone();
    let prior_owner = Arc::downgrade(&f.adapter.owner);
    let Fixture { adapter, original, input, faults, prepared, _directory: directory, .. } = f;
    drop(prepared);
    drop(input);
    drop(original);
    drop(adapter);
    drop(faults);
    assert!(prior_owner.upgrade().is_none(), "reopen must not retain the prior source/grant owner");
    for _ in 0..2 {
        let owner = Arc::new(ToolkitOriginalOwner::restore(
            Arc::new(meerkat_authorization::grants::FileGrantOwnerStore::new(directory.path().join("grants"))),
            Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
            association.candidate().ingress_namespace.ingress.clone()).unwrap());
        let source = private_originals_with_store(owner.clone(), Arc::new(
            toolkit_store::SqliteStore::open(directory.path().join("source.sqlite3")).unwrap())).await;
        let claims = association.candidate();
        assert!(owner.original_disposition(&claims.ingress_namespace, &claims.original_work).is_err(),
            "unknown source remains unavailable until canonical readback");
        source.select_installed_pair_recovery().unwrap();
        source.start_custody_recovery(None).unwrap();
        let observed = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if matches!(owner.original_disposition(&claims.ingress_namespace, &claims.original_work),
                    Ok(meerkat_runtime::meerkat_machine::OriginalDisposition::Active)) { break; }
                tokio::task::yield_now().await;
            }
        }).await;
        source.stop_custody_recovery();
        source.join_custody_recovery().await.unwrap();
        observed.unwrap();
        assert!(matches!(owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
            Ok(meerkat_runtime::meerkat_machine::RetainedCustody::NotRetained)));
        assert!(matches!(meerkat_runtime::meerkat_machine::require_source_original_live(
            owner.grants().as_ref(), owner.as_ref(), &claims.ingress_namespace,
            &claims.original_work, &claims.logical_executor, claims.represented_subject.as_ref()),
            Err(meerkat_runtime::input_authority::NativeAdmissionError::Refused(ref refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied));
        assert!(source.store.get(&withdrawn.key).await.unwrap().unwrap() == withdrawn);
        assert_eq!(source.binding_evidence(&reference).await.unwrap().as_ref(), Some(&association));
    }
}
