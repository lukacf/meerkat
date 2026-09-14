use super::*;
use crate::session::{DeferredToolResultsIngressError, SessionDeferredTurnState};
use crate::time_compat::SystemTime;
use crate::{AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn pending_fixture(
    scope: Option<RunEffectScopeId>,
) -> TestResult<(Session, PendingCallbackToolBatch, Vec<ToolResult>)> {
    pending_fixture_with_ids(scope, &["callback"])
}

fn pending_fixture_with_ids(
    scope: Option<RunEffectScopeId>,
    ids: &[&str],
) -> TestResult<(Session, PendingCallbackToolBatch, Vec<ToolResult>)> {
    let mut session = Session::new();
    session.push(Message::BlockAssistant(BlockAssistantMessage::new(
        ids.iter()
            .map(|id| {
                Ok(AssistantBlock::ToolUse {
                    id: (*id).into(),
                    name: "ask".into(),
                    args: serde_json::value::RawValue::from_string(
                        r#"{"question":"first"}"#.into(),
                    )?,
                    meta: None,
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
        StopReason::ToolUse,
    )));
    let batch = PendingCallbackToolBatch {
        run_id: RunId::new(),
        execution_scope: scope,
        execution_boundary: None,
        tool_use_order: ids.iter().map(|id| (*id).into()).collect(),
        pending_tool_use_ids: ids.iter().map(|id| (*id).into()).collect(),
        completed_results: Vec::new(),
        session_effects: Vec::new(),
        async_ops: Vec::new(),
    };
    session.stage_pending_callback_tool_batch(batch.clone())?;
    let results = ids
        .iter()
        .map(|id| ToolResult::new((*id).into(), "answer".into(), false))
        .collect();
    Ok((session, batch, results))
}

#[test]
fn callback_ingress_accepts_partial_evidence_without_applying_partial_results() -> TestResult {
    for scope in [
        None,
        Some(RunEffectScopeId::from_uuid(uuid::Uuid::new_v4())),
    ] {
        let (session, batch, results) = pending_fixture_with_ids(scope, &["first", "second"])?;
        let before = serde_json::to_vec(&session)?;
        for result in &results {
            assert_eq!(
                session.classify_callback_result_ingress(std::slice::from_ref(result))?,
                crate::session::CallbackResultIngress::Pending {
                    pending_tool_use_ids: batch.pending_tool_use_ids.clone()
                }
            );
            assert!(matches!(
                session.resolve_pending_callback_tool_results(vec![result.clone()]),
                Err(PendingCallbackBatchError::ResultSetMismatch { .. })
            ));
        }
        for invalid in [
            Vec::new(),
            vec![results[0].clone(), results[0].clone()],
            vec![ToolResult::new("foreign".into(), "answer".into(), false)],
        ] {
            assert!(session.classify_callback_result_ingress(&invalid).is_err());
        }
        assert_eq!(serde_json::to_vec(&session)?, before);
    }
    Ok(())
}

#[test]
fn callback_ingress_recognizes_identical_applied_subsets_and_refuses_conflicts() -> TestResult {
    let (mut session, batch, results) = pending_fixture_with_ids(None, &["first", "second"])?;
    session.commit_pending_callback_tool_results(&batch, results.clone(), Vec::new())?;
    let session: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    let before = serde_json::to_vec(&session)?;
    for result in &results {
        assert_eq!(
            session.classify_callback_result_ingress(std::slice::from_ref(result))?,
            crate::session::CallbackResultIngress::AlreadyApplied
        );
    }
    for invalid in [
        Vec::new(),
        vec![results[0].clone(), results[0].clone()],
        vec![ToolResult::new("foreign".into(), "answer".into(), false)],
        vec![ToolResult::new("first".into(), "different".into(), false)],
        vec![ToolResult::new("first".into(), "answer".into(), true)],
    ] {
        assert!(session.classify_callback_result_ingress(&invalid).is_err());
    }
    assert_eq!(serde_json::to_vec(&session)?, before);
    Ok(())
}

#[test]
fn callback_ingress_rejects_applied_receipt_order_corruption() -> TestResult {
    let (mut session, batch, results) = pending_fixture_with_ids(None, &["first", "second"])?;
    session.commit_pending_callback_tool_results(&batch, results.clone(), Vec::new())?;
    let mut document = serde_json::to_value(&session)?;
    document["metadata"][super::super::SESSION_PENDING_CALLBACK_BATCH_KEY]["results"]
        .as_array_mut()
        .ok_or("receipt results")?
        .reverse();
    let restored: Session = serde_json::from_value(document)?;
    assert!(
        restored
            .classify_callback_result_ingress(&results[..1])
            .is_err()
    );
    Ok(())
}

fn identity(session: &Session) -> TestResult<CallbackBatchIdentity> {
    Ok(
        match session
            .callback_batch_observation()?
            .ok_or("missing callback batch")?
        {
            CallbackBatchObservation::Pending { identity, .. }
            | CallbackBatchObservation::Applied { identity, .. } => identity,
        },
    )
}

#[test]
fn staged_callback_readiness_is_exact_ordered_and_restart_stable() -> TestResult {
    for scope in [
        None,
        Some(RunEffectScopeId::from_uuid(uuid::Uuid::new_v4())),
    ] {
        let (mut session, _, results) = pending_fixture_with_ids(scope, &["first", "second"])?;
        let target = identity(&session)?;
        assert_eq!(
            session.observe_staged_callback_results(&target)?,
            StagedCallbackResultsObservation::Incomplete {
                missing_tool_use_ids: vec!["first".into(), "second".into()],
            }
        );
        let mut deferred = SessionDeferredTurnState::default();
        session
            .prepare_callback_result_ingress(&results[1..], Some(&target))?
            .stage_into(&mut deferred, SystemTime::now())?;
        session.set_deferred_turn_state(deferred.clone())?;
        assert_eq!(
            session.observe_staged_callback_results(&target)?,
            StagedCallbackResultsObservation::Incomplete {
                missing_tool_use_ids: vec!["first".into()],
            }
        );
        session
            .prepare_callback_result_ingress(&results[..1], Some(&target))?
            .stage_into(&mut deferred, SystemTime::now())?;
        session.set_deferred_turn_state(deferred)?;
        let before = serde_json::to_vec(&session)?;
        let observation = session.observe_staged_callback_results(&target)?;
        let StagedCallbackResultsObservation::Complete(complete) = &observation else {
            return Err("full staged batch was not complete".into());
        };
        assert_eq!(complete.identity(), &target);
        assert_eq!(complete.ordered_results(), results);
        assert_eq!(serde_json::to_vec(&session)?, before);
        let restored: Session = serde_json::from_slice(&before)?;
        assert_eq!(
            restored.observe_staged_callback_results(&target)?,
            observation
        );

        let mut reversed = SessionDeferredTurnState::default();
        restored
            .prepare_callback_result_ingress(&results, Some(&target))?
            .stage_into(&mut reversed, SystemTime::now())?;
        session.set_deferred_turn_state(reversed.clone())?;
        assert_eq!(
            session.observe_staged_callback_results(&target)?,
            observation
        );
        reversed.pending_tool_results[0].results[0].is_error = true;
        session.set_deferred_turn_state(reversed)?;
        let StagedCallbackResultsObservation::Complete(changed) =
            session.observe_staged_callback_results(&target)?
        else {
            return Err("changed complete content was not observable".into());
        };
        assert_ne!(changed.digest(), complete.digest());
        if scope.is_some() {
            assert!(matches!(
                session.require_session_policy_callback_application(),
                Err(PendingCallbackBatchError::ScopedContinuationRequired)
            ));
        }
    }
    Ok(())
}

#[test]
fn staged_callback_readiness_rejects_unbound_foreign_duplicate_and_corrupt_data() -> TestResult {
    let (mut session, _, results) = pending_fixture_with_ids(None, &["first", "second"])?;
    let target = identity(&session)?;
    let mut staged = SessionDeferredTurnState::default();
    session
        .prepare_callback_result_ingress(&results, Some(&target))?
        .stage_into(&mut staged, SystemTime::now())?;
    let (other, _, _) = pending_fixture(None)?;
    for mutation in 0..5 {
        let mut invalid = staged.clone();
        match mutation {
            0 => invalid.pending_tool_results[0].callback_identity = None,
            1 => invalid.pending_tool_results[0].callback_identity = Some(identity(&other)?),
            2 => invalid.pending_tool_results[0].results[0].tool_use_id = "foreign".into(),
            3 => {
                let duplicate = invalid.pending_tool_results[0].clone();
                invalid.pending_tool_results.push(duplicate);
            }
            _ => invalid.pending_tool_results[0].results.clear(),
        }
        session.set_deferred_turn_state(invalid)?;
        let before = serde_json::to_vec(&session)?;
        assert!(session.observe_staged_callback_results(&target).is_err());
        assert_eq!(serde_json::to_vec(&session)?, before);
    }
    session.set_metadata_unchecked(
        super::super::SESSION_DEFERRED_TURN_STATE_KEY,
        serde_json::json!({"pending_tool_results": "corrupt"}),
    );
    assert!(matches!(
        session.observe_staged_callback_results(&target),
        Err(CallbackBatchObservationError::InvalidStagedResults(_))
    ));
    Ok(())
}

#[test]
fn staged_callback_readiness_includes_completed_siblings_but_never_reopens_applied_batch()
-> TestResult {
    let (mut session, mut batch, results) = pending_fixture_with_ids(None, &["local", "callback"])?;
    session.remove_metadata_unchecked(super::super::SESSION_PENDING_CALLBACK_BATCH_KEY);
    batch.pending_tool_use_ids = vec!["callback".into()];
    batch.completed_results = results[..1].to_vec();
    session.stage_pending_callback_tool_batch(batch.clone())?;
    let target = identity(&session)?;
    let mut deferred = SessionDeferredTurnState::default();
    session
        .prepare_callback_result_ingress(&results[1..], Some(&target))?
        .stage_into(&mut deferred, SystemTime::now())?;
    session.set_deferred_turn_state(deferred)?;
    let StagedCallbackResultsObservation::Complete(complete) =
        session.observe_staged_callback_results(&target)?
    else {
        return Err("callback plus completed sibling was incomplete".into());
    };
    assert_eq!(complete.ordered_results(), results);
    session.commit_pending_callback_tool_results(&batch, results, Vec::new())?;
    assert!(matches!(
        session.observe_staged_callback_results(&target)?,
        StagedCallbackResultsObservation::AlreadyApplied { results_digest, .. }
            if results_digest.as_ref() == Some(complete.digest())
    ));
    let reopened: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    assert_eq!(
        reopened.observe_staged_callback_results(&target)?,
        session.observe_staged_callback_results(&target)?
    );
    session.apply_pending_callback_resume_effects()?;
    assert!(matches!(
        session.observe_staged_callback_results(&target)?,
        StagedCallbackResultsObservation::AlreadyApplied {
            results_digest: Some(digest), resume_effects_applied: true,
        } if &digest == complete.digest()
    ));
    let mut legacy = serde_json::to_value(&session)?;
    legacy["metadata"]["session_pending_callback_batch_v1"]
        .as_object_mut()
        .ok_or("applied receipt")?
        .remove("complete_results_digest");
    let legacy: Session = serde_json::from_value(legacy)?;
    assert!(matches!(
        legacy.observe_staged_callback_results(&target)?,
        StagedCallbackResultsObservation::AlreadyApplied {
            results_digest: None,
            ..
        }
    ));
    session.push(session.messages()[0].clone());
    session.stage_pending_callback_tool_batch(PendingCallbackToolBatch {
        run_id: RunId::new(),
        ..batch
    })?;
    assert!(matches!(
        session.observe_staged_callback_results(&target),
        Err(CallbackBatchObservationError::TargetMismatch)
    ));
    Ok(())
}

#[test]
fn callback_staging_accumulates_only_exact_batch_members_after_reopen() -> TestResult {
    for scope in [
        None,
        Some(RunEffectScopeId::from_uuid(uuid::Uuid::new_v4())),
    ] {
        let (session, _, results) = pending_fixture_with_ids(scope, &["first", "second"])?;
        let target = identity(&session)?;
        let mut state = SessionDeferredTurnState::default();
        let first = session.prepare_callback_result_ingress(&results[..1], Some(&target))?;
        assert_eq!(first.stage_into(&mut state, SystemTime::now())?, 1);
        let encoded = serde_json::to_vec(&state)?;
        state = serde_json::from_slice(&encoded)?;
        assert_eq!(
            state.pending_tool_results()[0].callback_identity.as_ref(),
            Some(&target)
        );
        assert_eq!(first.stage_into(&mut state, SystemTime::now())?, 0);
        assert_eq!(serde_json::to_vec(&state)?, encoded);
        let complete = session.prepare_callback_result_ingress(&results, Some(&target))?;
        assert_eq!(complete.stage_into(&mut state, SystemTime::now())?, 1);
        let staged: Vec<_> = state
            .pending_tool_results()
            .iter()
            .flat_map(|message| message.results.clone())
            .collect();
        assert_eq!(staged, results);
        session.validate_deferred_callback_targets(state.pending_tool_results())?;

        let before = serde_json::to_vec(&state)?;
        let mut conflicting = results[..1].to_vec();
        conflicting[0].is_error = true;
        let conflicting = session.prepare_callback_result_ingress(&conflicting, Some(&target))?;
        assert!(matches!(
            conflicting.stage_into(&mut state, SystemTime::now()),
            Err(DeferredToolResultsIngressError::ConflictingRedelivery(_))
        ));
        assert_eq!(serde_json::to_vec(&state)?, before);
        if scope.is_some() {
            assert!(
                session
                    .prepare_callback_result_ingress(&results, None)
                    .is_err()
            );
        }
    }
    Ok(())
}

#[test]
fn callback_staging_refuses_wrong_target_and_unbound_staged_payloads() -> TestResult {
    let (session, _, results) = pending_fixture(None)?;
    let target = identity(&session)?;
    for (field, value) in [
        ("session_id", serde_json::to_value(SessionId::new())?),
        ("run_id", serde_json::to_value(RunId::new())?),
        (
            "execution_scope",
            serde_json::to_value(RunEffectScopeId::from_uuid(uuid::Uuid::new_v4()))?,
        ),
        (
            "execution_boundary",
            serde_json::to_value(crate::ops::OperationId::new())?,
        ),
        ("batch_digest", serde_json::json!(vec![0; 32])),
    ] {
        let mut wrong = serde_json::to_value(&target)?;
        wrong[field] = value;
        let wrong: CallbackBatchIdentity = serde_json::from_value(wrong)?;
        assert!(matches!(
            session.observe_staged_callback_results(&wrong),
            Err(CallbackBatchObservationError::TargetMismatch)
        ));
        assert!(
            session
                .prepare_callback_result_ingress(&results, Some(&wrong))
                .is_err(),
            "{field}"
        );
    }
    let mut unbound = SessionDeferredTurnState::default();
    unbound.try_stage_tool_results(results.clone(), SystemTime::now())?;
    let before = serde_json::to_vec(&unbound)?;
    let prepared = session.prepare_callback_result_ingress(&results, Some(&target))?;
    assert_eq!(
        prepared.stage_into(&mut unbound, SystemTime::now()),
        Err(DeferredToolResultsIngressError::CallbackTargetMismatch)
    );
    assert_eq!(serde_json::to_vec(&unbound)?, before);
    Ok(())
}

#[test]
fn callback_staged_target_survives_consumption_and_refuses_reused_call_ids() -> TestResult {
    let (mut session, batch, results) = pending_fixture(None)?;
    let target = identity(&session)?;
    let mut state = SessionDeferredTurnState::default();
    session
        .prepare_callback_result_ingress(&results, Some(&target))?
        .stage_into(&mut state, SystemTime::now())?;
    let consumed = state.consume_for_started_turn();
    session.validate_deferred_callback_targets(consumed.pending_tool_results())?;
    session.commit_pending_callback_tool_results(&batch, results, Vec::new())?;
    session.push(session.messages()[0].clone());
    session.stage_pending_callback_tool_batch(PendingCallbackToolBatch {
        run_id: RunId::new(),
        ..batch
    })?;
    let before = serde_json::to_vec(&session)?;
    assert!(
        session
            .validate_deferred_callback_targets(consumed.pending_tool_results())
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&session)?, before);
    let mut changed = consumed.pending_tool_results()[0].clone();
    changed.callback_identity = Some(identity(&session)?);
    assert_ne!(&changed, &consumed.pending_tool_results()[0]);
    Ok(())
}

#[test]
fn callback_identity_storage_ceiling_covers_optional_fields_and_digest_widths() -> TestResult {
    let ceiling = CallbackBatchIdentity::encoded_storage_byte_ceiling()?;
    let mut largest = 0;
    for scoped in [false, true] {
        for boundary in [false, true] {
            for byte in u8::MIN..=u8::MAX {
                let record: CallbackBatchIdentity = serde_json::from_value(serde_json::json!({
                    "session_id": SessionId::new(),
                    "run_id": RunId::new(),
                    "execution_scope": scoped.then(|| RunEffectScopeId::from_uuid(uuid::Uuid::new_v4())),
                    "execution_boundary": boundary.then(crate::ops::OperationId::new),
                    "batch_digest": vec![byte; 32],
                }))?;
                let bytes = serde_json::to_vec(&serde_json::to_string(&record)?)?;
                assert!(bytes.len() <= ceiling);
                largest = largest.max(bytes.len());
            }
        }
    }
    assert_eq!(largest, ceiling);
    Ok(())
}

#[test]
fn scoped_callback_effect_identity_requires_the_original_invocation_boundary() -> TestResult {
    let scope = RunEffectScopeId::from_uuid(uuid::Uuid::new_v4());
    let (session, mut batch, _) = pending_fixture(Some(scope))?;
    assert_eq!(
        identity(&session)?.scoped_tool_effect_id("callback"),
        Err(CallbackBatchObservationError::EffectIdentityUnavailable),
    );
    batch.execution_boundary = Some(crate::ops::OperationId::new());
    let first = session.pending_callback_identity(&batch)?;
    let effect = first
        .scoped_tool_effect_id("callback")?
        .ok_or("missing effect")?;
    assert_ne!(
        first.scoped_tool_effect_id("another")?,
        Some(effect.clone())
    );
    batch.execution_boundary = Some(crate::ops::OperationId::new());
    let second = session.pending_callback_identity(&batch)?;
    assert_ne!(first, second);
    assert_ne!(
        second.scoped_tool_effect_id("callback")?,
        Some(effect.clone())
    );
    let restored: CallbackBatchIdentity = serde_json::from_slice(&serde_json::to_vec(&first)?)?;
    assert_eq!(restored.scoped_tool_effect_id("callback")?, Some(effect));
    let (ordinary, _, _) = pending_fixture(None)?;
    assert_eq!(
        identity(&ordinary)?.scoped_tool_effect_id("callback")?,
        None
    );
    Ok(())
}

#[test]
fn callback_terminal_capture_requires_the_exact_pending_ids() -> TestResult {
    let (mut session, batch, results) = pending_fixture(None)?;
    let terminal = crate::AgentError::CallbackPending {
        tool_use_id: "callback".into(),
        tool_name: "host_callback_view".into(),
        args: serde_json::json!({"rendered_question": "first"}),
    };
    let expected = identity(&session)?;
    assert_eq!(
        session.callback_identity_for_terminal(&terminal)?,
        Some(expected.clone())
    );
    let batch_terminal = crate::AgentError::CallbackBatchPending {
        pending_tool_calls: vec![crate::error::PendingCallbackToolCall {
            tool_use_id: "callback".into(),
            tool_name: "host_callback_view".into(),
            args: serde_json::json!({"rendered_question": "first"}),
        }],
    };
    assert_eq!(
        session.callback_identity_for_terminal(&batch_terminal)?,
        Some(expected)
    );
    for wrong_ids in [Vec::new(), vec!["foreign"], vec!["callback", "callback"]] {
        let mismatch = crate::AgentError::CallbackBatchPending {
            pending_tool_calls: wrong_ids
                .into_iter()
                .map(|id| crate::error::PendingCallbackToolCall {
                    tool_use_id: id.into(),
                    tool_name: "host_callback_view".into(),
                    args: serde_json::json!({}),
                })
                .collect(),
        };
        assert_eq!(
            session.callback_identity_for_terminal(&mismatch),
            Err(CallbackBatchObservationError::TerminalMismatch),
        );
    }
    session.commit_pending_callback_tool_results(&batch, results, Vec::new())?;
    assert_eq!(
        session.callback_identity_for_terminal(&terminal),
        Err(CallbackBatchObservationError::TerminalMismatch),
    );
    assert_eq!(
        Session::new().callback_identity_for_terminal(&terminal)?,
        None
    );
    assert_eq!(
        session
            .callback_identity_for_terminal(&crate::AgentError::InternalError("failed".into()))?,
        None,
    );
    Ok(())
}

#[test]
fn callback_identity_survives_result_application_effects_and_serialization() -> TestResult {
    let (mut session, batch, results) = pending_fixture(None)?;
    let before = identity(&session)?;
    assert_eq!(before.run_id(), &batch.run_id);
    let reopened: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    assert_eq!(identity(&reopened)?, before);
    session.commit_pending_callback_tool_results(
        &batch,
        results,
        vec![Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: "after callback".into(),
                meta: None,
            }],
            StopReason::EndTurn,
        ))],
    )?;
    let mut reopened: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    assert_eq!(identity(&reopened)?, before);
    assert!(matches!(
        reopened.callback_batch_observation()?,
        Some(CallbackBatchObservation::Applied {
            resume_effects_applied: false,
            ..
        })
    ));
    reopened.apply_pending_callback_resume_effects()?;
    assert_eq!(identity(&reopened)?, before);
    assert!(matches!(
        reopened.callback_batch_observation()?,
        Some(CallbackBatchObservation::Applied {
            resume_effects_applied: true,
            ..
        })
    ));
    let after = serde_json::to_vec(&reopened)?;
    reopened.apply_pending_callback_resume_effects()?;
    assert_eq!(serde_json::to_vec(&reopened)?, after);
    Ok(())
}

#[test]
fn callback_identity_binds_actual_run_session_arguments_and_scope() -> TestResult {
    let (session, batch, _) = pending_fixture(None)?;
    let original = session.pending_callback_identity(&batch)?;
    let mut changed = batch.clone();
    changed.run_id = RunId::new();
    assert_ne!(session.pending_callback_identity(&changed)?, original);
    changed = batch.clone();
    changed.execution_scope = Some(RunEffectScopeId::from_uuid(uuid::Uuid::new_v4()));
    assert_ne!(session.pending_callback_identity(&changed)?, original);
    let mut foreign = session.clone();
    foreign.id = SessionId::new();
    assert_ne!(foreign.pending_callback_identity(&batch)?, original);
    let mut tail = session
        .messages()
        .last()
        .ok_or("missing assistant tail")?
        .clone();
    let Message::BlockAssistant(assistant) = &mut tail else {
        return Err("fixture must retain the actual assistant tool-use tail".into());
    };
    let AssistantBlock::ToolUse { args, .. } = &mut assistant.blocks[0] else {
        return Err("fixture must retain the callback arguments".into());
    };
    *args = serde_json::value::RawValue::from_string(r#"{"question":"second"}"#.into())?;
    let mut changed = Session::with_id(session.id().clone());
    changed.push(tail);
    changed.stage_pending_callback_tool_batch(batch.clone())?;
    let Message::BlockAssistant(actual_tail) =
        changed.messages().last().ok_or("missing changed tail")?
    else {
        return Err("changed fixture must contain the actual assistant tail".into());
    };
    assert_eq!(
        actual_tail
            .tool_calls()
            .next()
            .ok_or("missing actual tool call")?
            .args
            .get(),
        r#"{"question":"second"}"#,
    );
    assert_ne!(changed.pending_callback_identity(&batch)?, original);
    let mut with_sibling_effect = batch;
    with_sibling_effect
        .session_effects
        .push(crate::ops::SessionEffect::AppendAssistantBlocks {
            blocks: vec![AssistantBlock::Text {
                text: "sibling effect".into(),
                meta: None,
            }],
        });
    assert_ne!(
        session.pending_callback_identity(&with_sibling_effect)?,
        original
    );
    Ok(())
}

#[test]
fn callback_identity_does_not_invent_run_for_historical_applied_receipt() -> TestResult {
    let (mut session, batch, results) = pending_fixture(None)?;
    session.commit_pending_callback_tool_results(&batch, results.clone(), Vec::new())?;
    let mut document = serde_json::to_value(&session)?;
    document["metadata"][super::super::SESSION_PENDING_CALLBACK_BATCH_KEY]
        .as_object_mut()
        .ok_or("callback receipt must be an object")?
        .remove("identity");
    let historical: Session = serde_json::from_value(document)?;
    assert_eq!(
        historical.callback_batch_observation(),
        Err(CallbackBatchObservationError::AppliedIdentityUnavailable),
    );
    assert_eq!(
        historical.classify_callback_result_ingress(&results)?,
        super::super::CallbackResultIngress::AlreadyApplied,
    );
    Ok(())
}

#[test]
fn scoped_callback_cannot_apply_through_ordinary_owner_after_restore() -> TestResult {
    let scope = RunEffectScopeId::from_uuid(uuid::Uuid::new_v4());
    let (session, batch, results) = pending_fixture(Some(scope))?;
    let mut reopened: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    assert_eq!(identity(&reopened)?.execution_scope(), Some(scope));
    assert!(matches!(
        reopened.classify_callback_result_ingress(&results)?,
        super::super::CallbackResultIngress::Pending { .. }
    ));
    let before = serde_json::to_vec(&reopened)?;
    assert_eq!(
        reopened.require_session_policy_callback_continuation(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(
        reopened.require_session_policy_callback_application(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(
        reopened.commit_pending_callback_tool_results(&batch, results, Vec::new()),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(
        reopened.apply_pending_callback_resume_effects(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(serde_json::to_vec(&reopened)?, before);
    Ok(())
}

#[test]
fn callback_observation_refuses_corrupt_pending_and_foreign_applied_identity() -> TestResult {
    let (mut session, batch, results) = pending_fixture(None)?;
    let mut corrupt = session.clone();
    let mut invalid = batch.clone();
    invalid.pending_tool_use_ids.push("callback".into());
    corrupt.set_metadata_unchecked(
        super::super::SESSION_PENDING_CALLBACK_BATCH_KEY,
        serde_json::to_value(CallbackToolBatchState::Pending {
            batch: invalid,
            identity: None,
        })?,
    );
    assert!(matches!(
        corrupt.callback_batch_observation(),
        Err(CallbackBatchObservationError::InvalidBatch(_)),
    ));
    session.commit_pending_callback_tool_results(&batch, results, Vec::new())?;
    session.id = SessionId::new();
    assert_eq!(
        session.callback_batch_observation(),
        Err(CallbackBatchObservationError::ForeignSession),
    );
    Ok(())
}

#[test]
fn retained_callback_identity_refuses_rebinding_a_pending_batch() -> TestResult {
    let (mut session, mut batch, results) = pending_fixture(None)?;
    let retained = identity(&session)?;
    batch.run_id = RunId::new();
    session.set_metadata_unchecked(
        super::super::SESSION_PENDING_CALLBACK_BATCH_KEY,
        serde_json::to_value(CallbackToolBatchState::Pending {
            batch: batch.clone(),
            identity: Some(retained),
        })?,
    );
    let mut restored: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    let before = serde_json::to_vec(&restored)?;
    assert!(matches!(
        restored.callback_batch_observation(),
        Err(CallbackBatchObservationError::InvalidBatch(_)),
    ));
    assert!(restored.classify_callback_result_ingress(&results).is_err());
    assert!(
        restored
            .commit_pending_callback_tool_results(&batch, results, Vec::new())
            .is_err()
    );
    assert_eq!(serde_json::to_vec(&restored)?, before);
    Ok(())
}

#[test]
fn scoped_applied_receipt_cannot_restore_effects_under_session_policy() -> TestResult {
    let scope = RunEffectScopeId::from_uuid(uuid::Uuid::new_v4());
    let (mut session, mut batch, results) = pending_fixture(None)?;
    batch.execution_scope = Some(scope);
    let scoped_identity = session.pending_callback_identity(&batch)?;
    session.set_metadata_unchecked(
        super::super::SESSION_PENDING_CALLBACK_BATCH_KEY,
        serde_json::to_value(CallbackToolBatchState::Applied {
            identity: Some(scoped_identity.clone()),
            complete_results_digest: None,
            tool_use_order: batch.pending_tool_use_ids,
            results,
            async_ops: vec![crate::ops::AsyncOpRef::barrier(
                crate::ops::OperationId::new(),
            )],
            post_tool_messages: Vec::new(),
            post_tool_messages_applied: false,
        })?,
    );
    let mut reopened: Session = serde_json::from_slice(&serde_json::to_vec(&session)?)?;
    assert_eq!(identity(&reopened)?, scoped_identity);
    let before = serde_json::to_vec(&reopened)?;
    assert_eq!(
        reopened.require_session_policy_callback_continuation(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(
        reopened.require_session_policy_callback_application(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(
        reopened.apply_pending_callback_resume_effects(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
    );
    assert_eq!(serde_json::to_vec(&reopened)?, before);
    let Some(mut receipt) = reopened.callback_tool_batch_state()? else {
        return Err("missing applied fixture receipt".into());
    };
    let CallbackToolBatchState::Applied {
        post_tool_messages_applied,
        ..
    } = &mut receipt
    else {
        return Err("fixture must be applied".into());
    };
    *post_tool_messages_applied = true;
    reopened.set_metadata_unchecked(
        super::super::SESSION_PENDING_CALLBACK_BATCH_KEY,
        serde_json::to_value(receipt)?,
    );
    let before = serde_json::to_vec(&reopened)?;
    reopened.require_session_policy_callback_continuation()?;
    assert!(reopened.apply_pending_callback_resume_effects()?.is_empty());
    assert_eq!(
        reopened.require_session_policy_callback_application(),
        Err(PendingCallbackBatchError::ScopedContinuationRequired),
        "a historical receipt must not restore scoped async operations again",
    );
    assert_eq!(serde_json::to_vec(&reopened)?, before);
    Ok(())
}
