use super::*;
use meerkat_authorization_contracts::audit::{AuditObservation, SafeAuditObservation};
use meerkat_core::{OperationId, PrincipalKind, PrincipalRef, SessionId, TrustDomainId};

fn input_id() -> InputId {
    serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"").unwrap()
}
fn scope() -> OperationExecutionScope {
    OperationExecutionScope::RuntimeInput {
        owner_session_id: SessionId::parse("00000000-0000-0000-0000-000000000001").unwrap(),
        runtime_epoch_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap(),
        submitted_input_id: input_id(),
        canonical_input_id: input_id(),
    }
}
fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("audit-fixture").unwrap(),
    )
    .unwrap()
}
fn fixture() -> (InputAuthorizationAudit, NativeInputAuditSink) {
    let audit = InputAuthorizationAudit::default();
    let AuditPayload::Pending(records) = &audit.0 else {
        panic!("fresh pending payload");
    };
    // This fixture tests payload custody. Production obtains these projections
    // only through bind() over actual RetainedInputAuthority rows.
    let sink = NativeInputAuditSink {
        records: Arc::clone(records),
        execution_scope: scope(),
        run_id: RunId::new(),
        contributors: vec![NativeAuditContributor {
            input_id: input_id(),
            requester: principal("private-requester"),
            logical_executor: principal("private-executor"),
            represented_subject: Some(principal("private-subject")),
        }]
        .into(),
    };
    (audit, sink)
}
fn event(sink: &NativeInputAuditSink) -> AuthorizationAuditObservation {
    AuthorizationAuditObservation {
        operation_id: OperationId::new(),
        execution_scope: scope(),
        run_id: Some(sink.run_id.clone()),
        context_revision: None,
        observation: AuditObservation::Entry,
    }
}
fn count(audit: &InputAuthorizationAudit) -> usize {
    serde_json::to_value(audit)
        .unwrap()
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn frozen_prefix_rollback_clone_and_concurrent_suffix_do_not_drain_or_alias() {
    let (audit, sink) = fixture();
    let rollback = audit.clone();
    sink.append(event(&sink)).unwrap();
    let frozen = audit.freeze_for_persistence().unwrap();
    let bytes = serde_json::to_vec(&frozen).unwrap();
    std::thread::scope(|threads| {
        threads
            .spawn(|| sink.append(event(&sink)).unwrap())
            .join()
            .unwrap();
    });
    assert_eq!(serde_json::to_vec(&frozen).unwrap(), bytes);
    assert_eq!(count(&audit), 2);
    assert_eq!(count(&rollback), 2);
    assert_eq!(count(&frozen), 1);
    // Discarding a failed store candidate does not discard either observation.
    drop(frozen);
    assert_eq!(count(&rollback.freeze_for_persistence().unwrap()), 2);
}

#[test]
fn exact_scope_and_run_are_required_before_append() {
    let (audit, sink) = fixture();
    let mut wrong = event(&sink);
    wrong.execution_scope = OperationExecutionScope::Domain;
    assert_eq!(sink.append(wrong), Err(OperationObservationError));
    let mut wrong = event(&sink);
    wrong.run_id = Some(RunId::new());
    assert_eq!(sink.append(wrong), Err(OperationObservationError));
    assert!(audit.is_empty());
    assert!(
        audit
            .bind(&input_id(), scope(), sink.run_id.clone(), &[])
            .is_err()
    );
}

#[test]
fn installing_committed_prefix_preserves_live_sink_suffix_and_refuses_divergence() {
    let (live, sink) = fixture();
    sink.append(event(&sink)).unwrap();
    let committed = live.freeze_for_persistence().unwrap();
    sink.append(event(&sink)).unwrap();
    let installed = live.retain_live_with_committed(&committed).unwrap();
    sink.append(event(&sink)).unwrap();
    assert_eq!(count(&installed), 3);
    let (divergent, other_sink) = fixture();
    other_sink.append(event(&other_sink)).unwrap();
    assert!(live.retain_live_with_committed(&divergent).is_err());
    assert_eq!(count(&live), 3);
    assert_eq!(count(&divergent), 1);
    let empty_live = InputAuthorizationAudit::default();
    let restored_prefix = empty_live.retain_live_with_committed(&committed).unwrap();
    assert_eq!(count(&restored_prefix), 1);
}

#[test]
fn restored_payload_is_data_and_frozen_store_candidates_cannot_gain_live_sink() {
    let (audit, sink) = fixture();
    sink.append(event(&sink)).unwrap();
    let frozen = audit.freeze_for_persistence().unwrap();
    let restored = frozen.restore_observations_for_owner().unwrap();
    let AuditPayload::Pending(records) = &restored.0 else {
        panic!("restored payload");
    };
    records
        .lock()
        .unwrap()
        .push(StoredAuthorizationAuditObservation {
            contributors: Arc::clone(&sink.contributors),
            observation: event(&sink),
        });
    assert_eq!(count(&restored), 2);
    assert_eq!(count(&frozen), 1);
    assert!(
        frozen
            .bind(&input_id(), scope(), sink.run_id.clone(), &[])
            .is_err()
    );
}

#[test]
fn protected_parties_never_appear_in_debug_or_safe_projection() {
    let (audit, sink) = fixture();
    let event = event(&sink);
    assert_eq!(event.safe_projection(), SafeAuditObservation::EntryAttempt);
    sink.append(event.clone()).unwrap();
    let encoded = serde_json::to_string(&audit).unwrap();
    assert!(encoded.contains("private-requester")); // Protected native row only.
    assert!(!format!("{audit:?} {event:?}").contains("private-"));
    assert_eq!(
        serde_json::to_string(&event.safe_projection()).unwrap(),
        "\"entry_attempt\""
    );
}

#[tokio::test]
async fn existing_atomic_input_commit_persists_frozen_prefix_and_failed_cas_keeps_suffix() {
    use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
    use crate::store::{InMemoryRuntimeStore, RuntimeStore};
    let store = InMemoryRuntimeStore::new();
    let runtime = crate::identifiers::LogicalRuntimeId::new("audit-existing-row");
    let (audit, sink) = fixture();
    let mut row = StoredInputState::new_accepted(input_id());
    row.state.authorization_audit = audit.clone();
    sink.append(event(&sink)).unwrap();
    let record = InputStatePersistenceRecord::from_machine_snapshot(row.clone()).unwrap();
    sink.append(event(&sink)).unwrap();
    store
        .persist_input_states_atomically(&runtime, &[record])
        .await
        .unwrap();
    let committed = store
        .load_input_state(&runtime, &input_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count(&committed.state.authorization_audit), 1);
    let failed = InputStatePersistenceRecord::from_machine_snapshot(row.clone())
        .unwrap()
        .with_expected_row_digest("not-the-committed-row".into());
    assert!(
        store
            .persist_input_states_atomically(&runtime, &[failed])
            .await
            .is_err()
    );
    assert_eq!(count(&audit), 2);
    assert_eq!(
        count(
            &store
                .load_input_state(&runtime, &input_id())
                .await
                .unwrap()
                .unwrap()
                .state
                .authorization_audit
        ),
        1
    );
    let retry = InputStatePersistenceRecord::from_machine_snapshot(row).unwrap();
    store
        .persist_input_states_atomically(&runtime, &[retry])
        .await
        .unwrap();
    assert_eq!(
        count(
            &store
                .load_input_state(&runtime, &input_id())
                .await
                .unwrap()
                .unwrap()
                .state
                .authorization_audit
        ),
        2
    );
}

// These tests compose the real native sink with the shared prepared-entry seam.
// They do not authenticate work or claim a live native admission.
struct InfrastructureEntryPolicy {
    sink: Arc<dyn AuthorizationAuditSink>,
    event: AuthorizationAuditObservation,
}
struct InfrastructureEntryCheck {
    sink: Arc<dyn AuthorizationAuditSink>,
    event: AuthorizationAuditObservation,
    binding: meerkat_core::authorization::PreparedAuthorizationBinding,
}
impl meerkat_core::authorization::WorkAuthorization for InfrastructureEntryPolicy {
    fn prepare(
        &self,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<
        Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
        meerkat_core::OperationAuthorizationError,
    > {
        Ok(Arc::new(InfrastructureEntryCheck {
            sink: self.sink.clone(),
            event: self.event.clone(),
            binding: binding.clone(),
        }))
    }
}
impl meerkat_core::authorization::PreparedOperationAuthorization for InfrastructureEntryCheck {
    fn check_current(
        &self,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        assert!(self.binding.same_operation(binding));
        Ok(())
    }
    fn observe(
        &self,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        observation: meerkat_core::authorization::OperationObservation,
    ) -> Result<(), OperationObservationError> {
        assert!(self.binding.same_operation(binding));
        assert!(
            matches!(
                observation,
                meerkat_core::authorization::OperationObservation::Entry
            ),
            "entry infrastructure failure cannot recursively emit policy refusal or outcome"
        );
        self.sink.append(self.event.clone())
    }
}
fn infrastructure_entry_check(
    sink: Arc<dyn AuthorizationAuditSink>,
    event: AuthorizationAuditObservation,
) -> meerkat_core::authorization::PreparedOperationCheck {
    use meerkat_core::authorization::*;
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: event.operation_id.clone(),
        execution_scope: scope(),
        run_id: event.run_id.clone(),
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: principal("source-owner"),
                namespace: "fixture".into(),
                id: "source".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    PreparedOperationCheck::prepare(
        WorkAuthorizationContext::new(Arc::new(InfrastructureEntryPolicy { sink, event }), scope()),
        binding,
    )
    .unwrap()
}
fn assert_entry_infrastructure_and_independent_sibling(
    failed: meerkat_core::authorization::PreparedOperationCheck,
) {
    let error = failed
        .current()
        .unwrap()
        .observe_entry()
        .expect_err("no body may enter");
    let (other_audit, other_sink) = fixture();
    let other_event = event(&other_sink);
    let other = infrastructure_entry_check(Arc::new(other_sink), other_event);
    other.current().unwrap().observe_entry().unwrap();
    assert_eq!(count(&other_audit), 1, "independent sibling remains usable");
    assert_eq!(
        error, OperationObservationError,
        "pre-entry infrastructure failure must not become Denied"
    );
}

#[test]
fn observation_infrastructure_actual_native_poison_is_not_recovered_or_denied() {
    let (audit, sink) = fixture();
    let observation = event(&sink);
    let AuditPayload::Pending(records) = &audit.0 else {
        panic!("pending");
    };
    let records = records.clone();
    assert!(
        std::thread::spawn(move || {
            let _held = records.lock().unwrap();
            panic!("intentional audit mutex poison");
        })
        .join()
        .is_err()
    );
    assert!(
        audit.freeze_for_persistence().is_err(),
        "poison is not repaired via into_inner"
    );
    assert_entry_infrastructure_and_independent_sibling(infrastructure_entry_check(
        Arc::new(sink),
        observation,
    ));
}

fn native_mismatch_is_infrastructure(wrong_run: bool) {
    let (audit, sink) = fixture();
    let mut observation = event(&sink);
    if wrong_run {
        observation.run_id = Some(RunId::new());
    } else {
        observation.execution_scope = OperationExecutionScope::Domain;
    }
    let failed = infrastructure_entry_check(Arc::new(sink), observation);
    assert_entry_infrastructure_and_independent_sibling(failed);
    assert_eq!(count(&audit), 0);
}
#[test]
fn observation_infrastructure_actual_native_scope_mismatch_is_not_denial() {
    native_mismatch_is_infrastructure(false);
}
#[test]
fn observation_infrastructure_actual_native_run_mismatch_is_not_denial() {
    native_mismatch_is_infrastructure(true);
}

#[test]
fn observation_infrastructure_reserve_error_injection_is_not_policy_denial() {
    struct ReserveFailure;
    impl AuthorizationAuditSink for ReserveFailure {
        fn append(
            &self,
            _: AuthorizationAuditObservation,
        ) -> Result<(), OperationObservationError> {
            // A deterministic capacity-overflow error at the observation seam,
            // not global allocator exhaustion or proof of the native Vec branch.
            let mut bytes = Vec::<u8>::new();
            bytes
                .try_reserve(usize::MAX)
                .map_err(|_| OperationObservationError)?;
            panic!("usize::MAX reserve must fail");
        }
    }
    let (_, sink) = fixture();
    assert_entry_infrastructure_and_independent_sibling(infrastructure_entry_check(
        Arc::new(ReserveFailure),
        event(&sink),
    ));
}
