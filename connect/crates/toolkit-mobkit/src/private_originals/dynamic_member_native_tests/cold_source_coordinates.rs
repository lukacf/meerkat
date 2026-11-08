//! Real installed-owner close/reopen without an eager materialization read.
//! Model transport is inherited from Fixture; registration, source, SQLite,
//! governed submission, callback dispatch and cleanup use their actual owners.
use super::native_operations_tests::{add, native_input_states, ready};
use super::*;
use futures::FutureExt;

fn event(f: &Fixture, label: &str) -> PrivateAuthenticatedEvent {
    let mut event = private_event(label);
    event.route = f.route(ADDED).id;
    event.content_sha256 = digest(PROMPT.as_bytes());
    event
}

async fn submit(
    f: &Fixture,
    session: &meerkat_core::SessionId,
    original: PrivateOriginalRef,
    authentication: PrivateSourceEvidence,
    phase: &'static str,
) {
    let admitted = f
        .source
        .with_synthetic_admission_diagnostics(f.source.submit_authenticated_original(
            PrivateOriginalSubmit {
                original: original.clone(),
                content: PROMPT.into(),
                transport_authentication: authentication,
            },
        ))
        .await
        .unwrap();
    let PrivateOriginalSubmission::Admitted {
        input_id,
        turn_ticket: Some(ticket),
        deduplicated: false,
        source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
        ..
    } = admitted
    else {
        panic!("{phase}: one fresh ordinary input must be admitted and promoted");
    };
    let outcome = f
        .native
        .identity_runtime()
        .unwrap()
        .wait_for_turn(
            &AgentIdentity::parse(ADDED).unwrap(),
            TurnTicket::parse(&ticket).unwrap(),
            WAIT,
        )
        .await;
    assert!(
        matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER)
    );
    let native_input = meerkat_core::InputId::from_uuid(uuid::Uuid::parse_str(&input_id).unwrap());
    let stored = native_input_states(f, session)
        .await
        .into_iter()
        .find(|row| row.state.input_id == native_input)
        .expect("actual native input history");
    use meerkat_authorization_contracts::audit::{
        AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
    };
    use meerkat_core::authorization::OperationObservedOutcome;
    let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(stored).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    let ids = audit.iter().filter_map(|row| match &row.observation.observation {
        AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Tool { tool_name, .. } if tool_name == CALLBACK)
            => Some(row.observation.operation_id.clone()),
        _ => None,
    }).collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 1);
    assert!(
        audit
            .iter()
            .any(|row| ids.contains(&row.observation.operation_id)
                && matches!(
                    &row.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                ))
    );
    assert!(
        audit
            .iter()
            .all(|row| !ids.contains(&row.observation.operation_id)
                || !matches!(
                    &row.observation.observation,
                    AuditObservation::Refused { .. }
                        | AuditObservation::AuthorizationUnavailable { .. }
                ))
    );
    assert!(
        f.source
            .binding_evidence(&original)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_ordinary_source_retains_without_wake_then_uses_one_governed_input() {
    let fixture = Box::pin(Fixture::start_durable_with_recorder(class())).await;
    let prepared = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 6,
        Box::pin(async {
            let session = add(&fixture, ADDED).await;
            ready(&fixture, &session).await;
            let before = event(&fixture, "cold-source-before");
            let authentication = before.original_authentication.clone();
            let original = fixture.source.retain_event(before).await.unwrap();
            submit(
                &fixture,
                &session,
                original.clone(),
                authentication.clone(),
                "preparation",
            )
            .await;
            let cancelled = event(&fixture, "cold-source-cancelled");
            let cancelled_authentication = cancelled.original_authentication.clone();
            let cancelled = fixture.source.retain_event(cancelled).await.unwrap();
            fixture
                .source
                .record_operation_cancelled(&cancelled)
                .await
                .unwrap();
            let admission = fixture
                .policy
                .member_class_admission(&instance(ADDED).member.member)
                .unwrap()
                .unwrap();
            (
                session,
                original,
                authentication,
                cancelled,
                cancelled_authentication,
                admission,
            )
        }),
    ))
    .catch_unwind()
    .await;
    let (session, old_original, old_authentication, cancelled, cancelled_authentication, admission) =
        match prepared {
            Ok(Ok(value)) => value,
            failed => {
                fixture.close().await;
                match failed {
                    Err(panic) => std::panic::resume_unwind(panic),
                    _ => panic!("cold source preparation timed out"),
                }
            }
        };
    let old_source = Arc::downgrade(&fixture.source);
    let old_lifetime = Arc::downgrade(&fixture.lifetime);
    let old_factory = Arc::downgrade(&fixture.factory);
    let old_machine = Arc::downgrade(&fixture.machine);
    // Existing close joins the native/gateway owners before their actual drop.
    let fixture = fixture.reopen_with_recorder().await;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 6,
        Box::pin(async {
            let f = &fixture;
            assert!(
                old_source.upgrade().is_none()
                    && old_lifetime.upgrade().is_none()
                    && old_factory.upgrade().is_none()
                    && old_machine.upgrade().is_none()
            );
            assert_eq!(
                f.policy
                    .member_class_admission(&instance(ADDED).member.member)
                    .unwrap(),
                Some(admission)
            );
            let status = f
                .native
                .identity_runtime()
                .unwrap()
                .status(&AgentIdentity::parse(ADDED).unwrap())
                .await
                .unwrap();
            assert_eq!(status.session_id.as_ref(), Some(&session));
            assert_eq!(f.factory.preparations.load(Ordering::SeqCst), 0);
            assert!(f.factory.calls.lock().unwrap().is_empty());
            assert_eq!(f.provider.0.load(Ordering::SeqCst), 0);
            assert!(f.source.member_route(&f.route(ADDED).id).is_err());
            assert!(f.source.selected_source_route(&f.route(ADDED).id).is_err());
            assert!(
                !f.source
                    .current_routes()
                    .unwrap()
                    .iter()
                    .any(|row| row.id == f.route(ADDED).id)
            );
            let before_rows = native_input_states(f, &session).await;
            assert_eq!(before_rows.len(), 1);
            // Bound history and real cancellation are not cold-first-input permission.
            for (original, transport_authentication) in [
                (old_original, old_authentication),
                (cancelled, cancelled_authentication),
            ] {
                assert!(matches!(
                    f.source
                        .submit_authenticated_original(PrivateOriginalSubmit {
                            original,
                            content: PROMPT.into(),
                            transport_authentication,
                        })
                        .await,
                    Err(PrivateOriginalError::Denied)
                ));
            }
            let after = event(f, "cold-source-after");
            let authentication = after.original_authentication.clone();
            let original = f.source.retain_event(after).await.unwrap();
            let coordinates = f
                .source
                .cold_ordinary_source_coordinates(&original.route)
                .unwrap()
                .unwrap();
            assert!(
                f.source.key(&original).is_err(),
                "generic key remains strict"
            );
            assert!(
                f.source.event(&original).await.is_err(),
                "generic event remains strict"
            );
            assert!(
                f.source.original_event_key(&original).await.is_err(),
                "generic idempotency remains strict"
            );
            let key = f
                .source
                .event_key_for_source_coordinates(&original, &coordinates)
                .await
                .unwrap();
            assert_eq!(
                key,
                f.source
                    .event_key_for_source_coordinates(&original, &coordinates)
                    .await
                    .unwrap()
            );
            assert!(matches!(
                f.source
                    .submit_authenticated_original(PrivateOriginalSubmit {
                        original: original.clone(),
                        content: "changed bytes".into(),
                        transport_authentication: authentication.clone(),
                    })
                    .await,
                Err(PrivateOriginalError::Conflict)
            ));
            assert_eq!(
                f.factory.preparations.load(Ordering::SeqCst),
                0,
                "retention/read/refusal cannot wake"
            );
            assert_eq!(f.provider.0.load(Ordering::SeqCst), 0);
            assert!(f.factory.calls.lock().unwrap().is_empty());
            assert_eq!(native_input_states(f, &session).await.len(), 1);
            // No ready(), materialize(), extra input or actor lease after reopen.
            submit(f, &session, original, authentication, "restored").await;
            assert_eq!(f.factory.preparations.load(Ordering::SeqCst), 1);
            assert_eq!(f.factory.calls.lock().unwrap().as_slice(), [CALLBACK]);
            assert_eq!(f.provider.0.load(Ordering::SeqCst), 2);
            let rows = native_input_states(f, &session).await;
            assert_eq!(rows.len(), 2);
            assert!(
                rows.iter()
                    .any(|row| row.state.input_id == before_rows[0].state.input_id)
            );
            assert!(
                f.source.member_route(&f.route(ADDED).id).unwrap().is_some(),
                "only actual wake supplies live registration"
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.close().await;
    match checked {
        Ok(Ok(())) => (),
        Err(panic) => std::panic::resume_unwind(panic),
        _ => panic!("cold source input timed out"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_ordinary_source_refuses_revoked_policy_and_closed_owner_without_wake() {
    for closed in [false, true] {
        let fixture = Box::pin(Fixture::start_durable_with_recorder(class())).await;
        let prepared =
            std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 6, add(&fixture, ADDED)))
                .catch_unwind()
                .await;
        let session = match prepared {
            Ok(Ok(session)) => session,
            failed => {
                fixture.close().await;
                match failed {
                    Err(panic) => std::panic::resume_unwind(panic),
                    _ => panic!("cold denial Add timed out"),
                }
            }
        };
        let fixture = fixture.reopen_with_recorder().await;
        let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT * 6,
            Box::pin(async {
                let f = &fixture;
                let selected = event(f, "cold-denial");
                let authentication = selected.original_authentication.clone();
                let original = f.source.retain_event(selected).await.unwrap();
                if closed {
                    f.source.stop_custody_recovery();
                } else {
                    f._current.store(false, Ordering::SeqCst);
                }
                assert!(
                    f.source
                        .retain_event(event(f, "cold-denial-new"))
                        .await
                        .is_err()
                );
                assert!(
                    f.source
                        .submit_authenticated_original(PrivateOriginalSubmit {
                            original,
                            content: PROMPT.into(),
                            transport_authentication: authentication,
                        })
                        .await
                        .is_err()
                );
                assert_eq!(f.factory.preparations.load(Ordering::SeqCst), 0);
                assert!(f.factory.calls.lock().unwrap().is_empty());
                assert_eq!(f.provider.0.load(Ordering::SeqCst), 0);
                assert!(native_input_states(f, &session).await.is_empty());
            }),
        ))
        .catch_unwind()
        .await;
        fixture.close().await;
        match checked {
            Ok(Ok(())) => (),
            Err(panic) => std::panic::resume_unwind(panic),
            _ => panic!("cold denial timed out"),
        }
    }
}
