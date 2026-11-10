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
        review_attribution: None,
        delegate_attribution: None,
        observation: AuditObservation::Entry {
            boundary: Some(meerkat_core::OperationEntryBoundary::ToolDispatch),
        },
    }
}
fn count(audit: &InputAuthorizationAudit) -> usize {
    serde_json::to_value(audit)
        .unwrap()
        .as_array()
        .unwrap()
        .len()
}

/// Stage one refusal on an actual input row's live audit, as an operation of
/// `run_id` does after the row's last durable write: through the sink
/// [`InputAuthorizationAudit::bind`] derives for the row under a governed
/// original for it, with no row write of its own. Returns the observation's
/// operation id.
pub(crate) fn stage_refusal_after_row_write(
    audit: &InputAuthorizationAudit,
    row_input_id: &InputId,
    owner_session_id: &SessionId,
    run_id: &RunId,
) -> OperationId {
    let mut governed = crate::input_authority::tests::input("late-observation-requester");
    governed.header_mut().id = row_input_id.clone();
    let original = RetainedInputAuthority::from_input(&governed)
        .unwrap()
        .expect("the governed fixture input carries its authority association");
    let execution_scope = OperationExecutionScope::RuntimeInput {
        owner_session_id: owner_session_id.clone(),
        runtime_epoch_id: meerkat_core::RuntimeEpochId::from_uuid(uuid::Uuid::from_u128(7)),
        submitted_input_id: row_input_id.clone(),
        canonical_input_id: row_input_id.clone(),
    };
    let sink = audit
        .bind(
            row_input_id,
            execution_scope.clone(),
            run_id.clone(),
            std::slice::from_ref(&original),
            false,
        )
        .expect("the actual row binds its sink under its governed original");
    let operation_id = OperationId::new();
    sink.append(AuthorizationAuditObservation {
        operation_id: operation_id.clone(),
        execution_scope,
        run_id: Some(run_id.clone()),
        context_revision: None,
        review_attribution: None,
        delegate_attribution: None,
        observation: AuditObservation::Refused {
            target: Arc::new(meerkat_authorization_contracts::audit::AuditTarget::Tool {
                call_id: "call-after-row-write".into(),
                tool_name: "late_tool".into(),
                arguments_digest:
                    meerkat_authorization_contracts::evidence::EvidenceDigest::from_array([5; 32]),
                owners: Vec::new(),
            }),
            reason: OperationRefusalKind::MalformedFacts,
            read: None,
        },
    })
    .expect("the bound sink stages the refusal on the live row");
    operation_id
}

/// The operation ids of a stored row's audit, in order.
pub(crate) fn audit_operation_ids(audit: &InputAuthorizationAudit) -> Vec<OperationId> {
    serde_json::from_value::<Vec<StoredAuthorizationAuditObservation>>(
        serde_json::to_value(audit).unwrap(),
    )
    .unwrap()
    .into_iter()
    .map(|record| record.observation.operation_id)
    .collect()
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
            .bind(&input_id(), scope(), sink.run_id.clone(), &[], false)
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
fn committed_image_is_only_the_stored_prefix_of_the_live_records() {
    let bytes = |audit: &InputAuthorizationAudit| serde_json::to_vec(audit).unwrap();
    let (live, sink) = fixture();
    sink.append(event(&sink)).unwrap();
    let stored = live.freeze_for_persistence().unwrap();
    sink.append(event(&sink)).unwrap();
    // The live records ran ahead of the stored ones by a suffix: the image is
    // the stored prefix, and the live buffer keeps the suffix.
    let image = live.committed_image(&stored).unwrap();
    assert_eq!(bytes(&image), bytes(&stored));
    assert_eq!(count(&live), 2);
    // The image is frozen: a later append reaches only the live buffer.
    sink.append(event(&sink)).unwrap();
    assert_eq!(count(&image), 1);
    assert_eq!(count(&live), 3);
    // Every other relation leaves the live records, which an exact compare
    // against the stored row refuses: equal records,
    let current = live.freeze_for_persistence().unwrap();
    assert_eq!(
        bytes(&live.committed_image(&current).unwrap()),
        bytes(&current)
    );
    // records that diverge from the live ones,
    let (other, other_sink) = fixture();
    other_sink.append(event(&other_sink)).unwrap();
    assert_eq!(
        bytes(&live.committed_image(&other).unwrap()),
        bytes(&current)
    );
    // and stored records ahead of the live ones.
    let behind = stored.restore_observations_for_owner().unwrap();
    assert_eq!(
        bytes(&behind.committed_image(&current).unwrap()),
        bytes(&stored)
    );
    // Neither side changes.
    assert_eq!(count(&live), 3);
    assert_eq!(count(&other), 1);
    assert_eq!(count(&stored), 1);
}

#[test]
fn same_records_compares_every_record_in_order() {
    let (live, sink) = fixture();
    sink.append(event(&sink)).unwrap();
    let stored = live.freeze_for_persistence().unwrap();
    // A live buffer and a frozen copy of it hold the same records.
    assert!(live.has_same_records(&stored).unwrap());
    assert!(stored.has_same_records(&live).unwrap());
    // A record one side lacks is a difference, in either direction,
    sink.append(event(&sink)).unwrap();
    assert!(!live.has_same_records(&stored).unwrap());
    assert!(!stored.has_same_records(&live).unwrap());
    // and so are different records of the same count.
    let (other, other_sink) = fixture();
    other_sink.append(event(&other_sink)).unwrap();
    assert!(!other.has_same_records(&stored).unwrap());
    // Neither side changes.
    assert_eq!(count(&live), 2);
    assert_eq!(count(&stored), 1);
    assert_eq!(count(&other), 1);
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
            .bind(&input_id(), scope(), sink.run_id.clone(), &[], false)
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
    allow_outcome: bool,
}
struct InfrastructureEntryCheck {
    sink: Arc<dyn AuthorizationAuditSink>,
    event: AuthorizationAuditObservation,
    binding: meerkat_core::authorization::PreparedAuthorizationBinding,
    allow_outcome: bool,
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
            allow_outcome: self.allow_outcome,
        }))
    }
}
impl meerkat_core::authorization::PreparedOperationAuthorization for InfrastructureEntryCheck {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }

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
        let mut event = self.event.clone();
        match observation {
            meerkat_core::authorization::OperationObservation::Entry(_) => {}
            meerkat_core::authorization::OperationObservation::Outcome(outcome)
                if self.allow_outcome =>
            {
                event.observation = AuditObservation::Outcome { outcome };
            }
            _ => panic!(
                "entry infrastructure failure cannot recursively emit policy refusal or outcome"
            ),
        }
        self.sink.append(event)
    }

    fn spend_at_entry(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        Ok(())
    }
}
fn infrastructure_entry_check(
    sink: Arc<dyn AuthorizationAuditSink>,
    event: AuthorizationAuditObservation,
) -> meerkat_core::authorization::PreparedOperationCheck {
    infrastructure_check(sink, event, false)
}
fn infrastructure_check(
    sink: Arc<dyn AuthorizationAuditSink>,
    event: AuthorizationAuditObservation,
    allow_outcome: bool,
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
        WorkAuthorizationContext::new(
            Arc::new(InfrastructureEntryPolicy {
                sink,
                event,
                allow_outcome,
            }),
            scope(),
        ),
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
        .observe_entry(meerkat_core::OperationEntryBoundary::ToolDispatch)
        .expect_err("no body may enter");
    let (other_audit, other_sink) = fixture();
    let other_event = event(&other_sink);
    let other = infrastructure_entry_check(Arc::new(other_sink), other_event);
    other
        .current()
        .unwrap()
        .observe_entry(meerkat_core::OperationEntryBoundary::ToolDispatch)
        .unwrap();
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

/// A real native pending-buffer failure after an entry observation. The HTTP
/// status is an observation fixture: no request or physical effect runs here,
/// and a frozen prefix is a store candidate, not proof of a durable commit.
#[test]
fn observation_infrastructure_native_poison_after_entry_preserves_frozen_prefix() {
    use meerkat_core::authorization::OperationObservedOutcome;

    let (audit, sink) = fixture();
    let observation = event(&sink);
    let check = infrastructure_check(Arc::new(sink), observation.clone(), true);
    let entered = check.current().unwrap();
    entered
        .observe_entry(meerkat_core::OperationEntryBoundary::ToolDispatch)
        .unwrap();
    let frozen = audit.freeze_for_persistence().unwrap();
    let prefix = serde_json::to_vec(&frozen).unwrap();
    let prior: Vec<StoredAuthorizationAuditObservation> = serde_json::from_slice(&prefix).unwrap();
    assert_eq!(prior.len(), 1);
    assert_eq!(prior[0].observation.operation_id, observation.operation_id);
    assert!(matches!(
        prior[0].observation.observation,
        AuditObservation::Entry { .. }
    ));

    let AuditPayload::Pending(records) = &audit.0 else {
        panic!("same live pending buffer");
    };
    let poisoned_records = Arc::clone(records);
    assert!(
        std::thread::spawn(move || {
            let _held = poisoned_records.lock().unwrap();
            panic!("intentional audit mutex poison after entry");
        })
        .join()
        .is_err()
    );

    let error = entered
        .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
        .expect_err("the retained check cannot stage an outcome into its poisoned native sink");
    assert_eq!(error, OperationObservationError);
    assert!(
        audit.freeze_for_persistence().is_err(),
        "live poison stays unavailable"
    );
    assert_eq!(serde_json::to_vec(&frozen).unwrap(), prefix);
    // Inspect the poisoned bytes only as a test oracle. Do not restore an
    // appendable owner or clear poison to turn the failed outcome into success.
    let poisoned = records
        .lock()
        .expect_err("the same pending mutex remains poisoned");
    assert_eq!(
        serde_json::to_vec(&**poisoned.get_ref()).unwrap(),
        prefix,
        "the failed outcome cannot append an Outcome, Refused, or replacement Entry"
    );
    drop(poisoned);

    let (other_audit, other_sink) = fixture();
    let other_event = event(&other_sink);
    let other = infrastructure_check(Arc::new(other_sink), other_event.clone(), true);
    let other_entered = other.current().unwrap();
    other_entered
        .observe_entry(meerkat_core::OperationEntryBoundary::ToolDispatch)
        .unwrap();
    other_entered
        .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
        .unwrap();
    let other_records: Vec<StoredAuthorizationAuditObservation> =
        serde_json::from_value(serde_json::to_value(&other_audit).unwrap()).unwrap();
    assert_eq!(
        other_records.len(),
        2,
        "the independent native sink remains usable"
    );
    assert_eq!(
        other_records[1].observation.operation_id,
        other_event.operation_id
    );
    assert!(matches!(
        other_records[0].observation.observation,
        AuditObservation::Entry { .. }
    ));
    assert!(matches!(
        other_records[1].observation.observation,
        AuditObservation::Outcome {
            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
        }
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

/// A mandatory-hook withholding staged on the actual row persists through the
/// existing atomic input commit with its candidate id and retained
/// contributors, is written at the current stored input state version, and
/// cannot appear in a row stamped before v6.
#[tokio::test]
async fn mandatory_hook_withholding_persists_on_the_actual_row_at_the_current_version() {
    use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
    use crate::store::{InMemoryRuntimeStore, RuntimeStore};
    use meerkat_core::generated::session_persistence_version_authority::stored_input_state_version_for_row;
    let store = InMemoryRuntimeStore::new();
    let runtime = crate::identifiers::LogicalRuntimeId::new("audit-hook-withholding");
    let (audit, sink) = fixture();
    let mut row = StoredInputState::new_accepted(input_id());
    row.state.authorization_audit = audit.clone();
    let candidate = OperationId::new();
    let withholding = meerkat_core::MandatoryHookWithholding {
        sequence: 1,
        hook_id: Some(meerkat_core::HookId::new("gate")),
        hook_id_omitted: false,
        point: meerkat_core::HookPoint::PreLlmRequest,
        cause: meerkat_core::MandatoryHookCause::Denied {
            reason_code: meerkat_core::HookReasonCode::PolicyViolation,
        },
        effect: meerkat_core::MandatoryHookWithheldEffect::RequestNotSent,
    };
    sink.append(AuthorizationAuditObservation {
        operation_id: candidate.clone(),
        observation: AuditObservation::MandatoryHookWithheld {
            withholding: withholding.clone(),
        },
        ..event(&sink)
    })
    .unwrap();
    assert!(audit.has_mandatory_hook_withholding());
    let record = InputStatePersistenceRecord::from_machine_snapshot(row).unwrap();
    store
        .persist_input_states_atomically(&runtime, &[record])
        .await
        .unwrap();
    let committed = store
        .load_input_state(&runtime, &input_id())
        .await
        .unwrap()
        .unwrap();
    let stored = serde_json::to_value(&committed).unwrap();
    // The row carries no requeue fact, so it is written at the baseline.
    assert_eq!(
        stored["stored_input_state_version"],
        stored_input_state_version_for_row(false)
    );
    let records = serde_json::to_value(&committed.state.authorization_audit).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(
        records[0]["observation"]["operation_id"],
        serde_json::json!(candidate)
    );
    assert_eq!(
        records[0]["observation"]["observation"]["kind"],
        "mandatory_hook_withheld"
    );
    assert_eq!(records[0]["contributors"].as_array().unwrap().len(), 1);
    let reread: StoredInputState = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(&reread).unwrap(), stored);

    let mut older = stored.clone();
    older["stored_input_state_version"] = serde_json::json!(5);
    assert!(
        serde_json::from_value::<StoredInputState>(older).is_err(),
        "a pre-v6 row cannot carry the withholding record"
    );
    let mut unknown = stored;
    unknown["authorization_audit"][0]["observation"]["observation"]["kind"] =
        serde_json::json!("future_hook_kind");
    assert!(serde_json::from_value::<StoredInputState>(unknown).is_err());
}

/// A refusal read and a policy read's publication instance persist on the
/// actual row at the current stored input state version. A row stamped
/// before v7 that carries either one is refused; without them it still
/// reads.
#[tokio::test]
async fn publication_reads_persist_at_the_current_version_and_never_before_v7() {
    use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
    use crate::store::{InMemoryRuntimeStore, RuntimeStore};
    use meerkat_authorization_contracts::audit::{
        AuditPolicyRead, AuditPublicationMode, AuditRecipient, AuditRefusalBasis, AuditRefusalRead,
        AuditTarget,
    };
    use meerkat_core::generated::session_persistence_version_authority::stored_input_state_version_for_row;

    let store = InMemoryRuntimeStore::new();
    let runtime = crate::identifiers::LogicalRuntimeId::new("audit-publication-reads");
    let (audit, sink) = fixture();
    let mut row = StoredInputState::new_accepted(input_id());
    row.state.authorization_audit = audit.clone();
    let target = Arc::new(AuditTarget::Publication {
        recipient: AuditRecipient::Principal {
            principal: principal("recipient"),
        },
        mode: AuditPublicationMode::Buffered,
    });
    let instance = uuid::Uuid::new_v4();
    sink.append(AuthorizationAuditObservation {
        observation: AuditObservation::Refused {
            target: Arc::clone(&target),
            reason: OperationRefusalKind::ReprepareRequired,
            read: Some(AuditRefusalRead::new(
                1,
                AuditRefusalBasis::ChangeInProgress,
                3,
                Some(3),
                instance,
            )),
        },
        ..event(&sink)
    })
    .unwrap();
    sink.append(AuthorizationAuditObservation {
        observation: AuditObservation::Prepared {
            target,
            policy: AuditPolicyRead {
                publication_sequence: 4,
                observed_at_ms: 2,
                operation_authorities: Vec::new(),
                controller_lineages: Vec::new(),
                publication_instance: Some(instance),
            },
        },
        ..event(&sink)
    })
    .unwrap();
    assert!(audit.has_publication_read());
    let record = InputStatePersistenceRecord::from_machine_snapshot(row).unwrap();
    store
        .persist_input_states_atomically(&runtime, &[record])
        .await
        .unwrap();
    let committed = store
        .load_input_state(&runtime, &input_id())
        .await
        .unwrap()
        .unwrap();
    let stored = serde_json::to_value(&committed).unwrap();
    // The row carries no requeue fact, so it is written at the baseline.
    assert_eq!(
        stored["stored_input_state_version"],
        stored_input_state_version_for_row(false)
    );
    let reread: StoredInputState = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(serde_json::to_value(&reread).unwrap(), stored);

    let mut older = stored;
    older["stored_input_state_version"] = serde_json::json!(6);
    let mut only_instance = older.clone();
    only_instance["authorization_audit"][0]["observation"]["observation"]
        .as_object_mut()
        .unwrap()
        .remove("read")
        .unwrap();
    let mut only_read = older.clone();
    only_read["authorization_audit"][1]["observation"]["observation"]["policy"]
        .as_object_mut()
        .unwrap()
        .remove("publication_instance")
        .unwrap();
    let mut neither = only_instance.clone();
    neither["authorization_audit"][1]["observation"]["observation"]["policy"]
        .as_object_mut()
        .unwrap()
        .remove("publication_instance")
        .unwrap();
    for (row, carries) in [
        (older, "both"),
        (only_instance, "a publication instance"),
        (only_read, "a refusal read"),
    ] {
        assert!(
            serde_json::from_value::<StoredInputState>(row).is_err(),
            "a v6 row cannot carry {carries}"
        );
    }
    assert!(
        serde_json::from_value::<StoredInputState>(neither).is_ok(),
        "a v6 row without either still reads"
    );
}
