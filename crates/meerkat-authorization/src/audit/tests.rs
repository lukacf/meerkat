#![allow(clippy::expect_used, clippy::unwrap_used)]
use super::*;
use meerkat_core::authorization::{
    OperationAuthorizationFacts, OwnerQualifiedTarget, SourceAuthorizationFacts,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::{OperationId, PrincipalKind, PrincipalRef, TrustDomainId};

#[test]
fn protected_source_coordinates_are_exact_and_not_debug_output() {
    let owner = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "private-owner",
        TrustDomainId::new("private-domain").expect("domain"),
    )
    .expect("principal");
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: owner.clone(),
                namespace: "private-namespace".into(),
                id: "private-resource".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    let target = target(&binding);
    let AuditTarget::ExternalSource { resource, usage } = &target else {
        unreachable!()
    };
    assert_eq!(resource.domain.authority, owner);
    assert_eq!(resource.domain.namespace, "private-namespace");
    assert_eq!(resource.resource_id, "private-resource");
    assert!(matches!(usage, AuditSourceUse::Read));
    assert!(!format!("{target:?}").contains("private-"));
}

#[test]
fn unavailable_clock_never_records_a_false_policy_refusal() {
    struct FailingClock;
    impl LocalAuthorizationClock for FailingClock {
        fn now(
            &self,
        ) -> Result<crate::work::LocalAuthorizationTime, crate::clock::LocalClockError> {
            Err(crate::clock::LocalClockError::Unavailable)
        }
    }
    struct UnenteredPolicy;
    impl WorkAuthorization for UnenteredPolicy {
        #[allow(clippy::panic)] // Entering this policy is the failing test sentinel.
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            panic!("unavailable clock must prevent policy preparation")
        }
    }
    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<AuthorizationAuditObservation>>);
    impl AuthorizationAuditSink for Sink {
        fn append(
            &self,
            observation: AuthorizationAuditObservation,
        ) -> Result<(), OperationObservationError> {
            self.0.lock().expect("observations").push(observation);
            Ok(())
        }
    }
    let sink = Arc::new(Sink::default());
    let work = AuditedWorkAuthorization {
        inner: Arc::new(UnenteredPolicy),
        associations: Vec::new().into(),
        publication: LocalAuthorizationPublication::new(),
        clock: Arc::new(FailingClock),
        sink: sink.clone(),
    };
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: PrincipalRef::in_domain(
                    PrincipalKind::ServiceAccount,
                    "owner",
                    TrustDomainId::new("domain").expect("domain"),
                )
                .expect("principal"),
                namespace: "fixture".into(),
                id: "fixture".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    let error = work
        .prepare(&binding)
        .err()
        .expect("missing time cannot issue permission");
    let events = sink.0.lock().expect("observations");
    assert_eq!(events.len(), 1, "retain one exact attempt observation");
    assert_eq!(events[0].operation_id, binding.facts().operation_id);
    assert!(matches!(
        events[0].observation,
        AuditObservation::AuthorizationUnavailable { .. }
    ));
    assert_eq!(
        events[0].safe_projection(),
        meerkat_authorization_contracts::audit::SafeAuditObservation::AuthorizationUnavailable
    );
    let encoded = serde_json::to_value(&events[0]).expect("typed protected record");
    let decoded: AuthorizationAuditObservation =
        serde_json::from_value(encoded).expect("typed record round trip");
    assert_eq!(decoded, events[0]);
    assert_eq!(
        serde_json::to_value(decoded.safe_projection()).unwrap(),
        serde_json::json!("authorization_unavailable")
    );
    assert!(
        !matches!(events[0].observation, AuditObservation::Refused { .. }),
        "unavailable time is not an authoritative policy refusal"
    );
    assert!(!matches!(
        events[0].observation,
        AuditObservation::Prepared { .. }
            | AuditObservation::Entry { .. }
            | AuditObservation::Outcome { .. }
    ));
    assert_eq!(
        meerkat_core::ToolError::from(error).error_code(),
        "operation_authorization_unavailable"
    );
}

#[test]
fn observation_infrastructure_inner_failure_survives_concurrent_publication_change() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Clock;
    impl LocalAuthorizationClock for Clock {
        fn now(
            &self,
        ) -> Result<crate::work::LocalAuthorizationTime, crate::clock::LocalClockError> {
            Ok(crate::work::LocalAuthorizationTime {
                unix_ms: 1,
                monotonic: meerkat_core::time_compat::Instant::now(),
            })
        }
    }
    struct FailingInner {
        publication: LocalAuthorizationPublication,
        change_during_prepare: bool,
        calls: AtomicUsize,
    }
    impl WorkAuthorization for FailingInner {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.change_during_prepare {
                let mutation = self
                    .publication
                    .begin_owner_change()
                    .expect("actual publication");
                drop(mutation);
            }
            Err(OperationAuthorizationError::ObservationUnavailable(
                OperationObservationError,
            ))
        }
    }
    #[derive(Default)]
    struct Sink(AtomicUsize);
    impl AuthorizationAuditSink for Sink {
        fn append(
            &self,
            _: AuthorizationAuditObservation,
        ) -> Result<(), OperationObservationError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    for change_during_prepare in [false, true] {
        let publication = LocalAuthorizationPublication::new();
        let ((), before) = publication.observe(|| ()).expect("initial stamp");
        let inner = Arc::new(FailingInner {
            publication: publication.clone(),
            change_during_prepare,
            calls: AtomicUsize::new(0),
        });
        let sink = Arc::new(Sink::default());
        let work = AuditedWorkAuthorization {
            inner: inner.clone(),
            associations: Vec::new().into(),
            publication,
            clock: Arc::new(Clock),
            sink: sink.clone(),
        };
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId::new(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: PrincipalRef::in_domain(
                        PrincipalKind::ServiceAccount,
                        "owner",
                        TrustDomainId::new("domain").expect("domain"),
                    )
                    .expect("principal"),
                    namespace: "fixture".into(),
                    id: "fixture".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        });
        assert!(matches!(
            work.prepare(&binding),
            Err(OperationAuthorizationError::ObservationUnavailable(
                OperationObservationError
            ))
        ));
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            1,
            "no retry of failed infrastructure"
        );
        assert_eq!(
            sink.0.load(Ordering::SeqCst),
            0,
            "neither stable nor changed publication may recursively audit an infrastructure failure"
        );
        if change_during_prepare {
            assert_eq!(before.check_current(), Err(PublicationError::Changed));
        } else {
            assert_eq!(before.check_current(), Ok(()));
        }
    }
}

#[test]
fn authorization_unavailable_survives_publication_change_and_is_observed() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Clock;
    impl LocalAuthorizationClock for Clock {
        fn now(
            &self,
        ) -> Result<crate::work::LocalAuthorizationTime, crate::clock::LocalClockError> {
            Ok(crate::work::LocalAuthorizationTime {
                unix_ms: 1,
                monotonic: meerkat_core::time_compat::Instant::now(),
            })
        }
    }
    struct FailingInner {
        publication: LocalAuthorizationPublication,
        change_during_prepare: bool,
        calls: AtomicUsize,
    }
    impl WorkAuthorization for FailingInner {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.change_during_prepare {
                let mutation = self
                    .publication
                    .begin_owner_change()
                    .expect("actual publication");
                drop(mutation);
            }
            Err(OperationAuthorizationError::Unavailable)
        }
    }
    #[derive(Default)]
    struct Sink(AtomicUsize);
    impl AuthorizationAuditSink for Sink {
        fn append(
            &self,
            event: AuthorizationAuditObservation,
        ) -> Result<(), OperationObservationError> {
            assert!(matches!(
                event.observation,
                AuditObservation::AuthorizationUnavailable { .. }
            ));
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    for change_during_prepare in [false, true] {
        let publication = LocalAuthorizationPublication::new();
        let ((), before) = publication.observe(|| ()).expect("initial stamp");
        let inner = Arc::new(FailingInner {
            publication: publication.clone(),
            change_during_prepare,
            calls: AtomicUsize::new(0),
        });
        let sink = Arc::new(Sink::default());
        let work = AuditedWorkAuthorization {
            inner: inner.clone(),
            associations: Vec::new().into(),
            publication,
            clock: Arc::new(Clock),
            sink: sink.clone(),
        };
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: OperationId::new(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: PrincipalRef::in_domain(
                        PrincipalKind::ServiceAccount,
                        "owner",
                        TrustDomainId::new("domain").expect("domain"),
                    )
                    .expect("principal"),
                    namespace: "fixture".into(),
                    id: "fixture".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        });
        assert!(matches!(
            work.prepare(&binding),
            Err(OperationAuthorizationError::Unavailable)
        ));
        assert_eq!(
            inner.calls.load(Ordering::SeqCst),
            1,
            "no retry of failed infrastructure"
        );
        assert_eq!(
            sink.0.load(Ordering::SeqCst),
            1,
            "one unavailable observation, not a policy refusal"
        );
        if change_during_prepare {
            assert_eq!(before.check_current(), Err(PublicationError::Changed));
        } else {
            assert_eq!(before.check_current(), Ok(()));
        }
    }
}

/// The audit-only withholding path appends the typed record to the same
/// row-bound sink, under the work context's own scope, without any policy
/// read, clock or preparation.
#[test]
fn mandatory_hook_withholding_reaches_the_sink_without_policy_or_preparation() {
    struct FailingClock;
    impl LocalAuthorizationClock for FailingClock {
        fn now(
            &self,
        ) -> Result<crate::work::LocalAuthorizationTime, crate::clock::LocalClockError> {
            Err(crate::clock::LocalClockError::Unavailable)
        }
    }
    struct UnenteredPolicy;
    impl WorkAuthorization for UnenteredPolicy {
        #[allow(clippy::panic)] // Entering this policy is the failing test sentinel.
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            panic!("recording a hook withholding must not prepare an operation")
        }
    }
    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<AuthorizationAuditObservation>>);
    impl AuthorizationAuditSink for Sink {
        fn append(
            &self,
            observation: AuthorizationAuditObservation,
        ) -> Result<(), OperationObservationError> {
            self.0.lock().expect("observations").push(observation);
            Ok(())
        }
    }
    let sink = Arc::new(Sink::default());
    let audited = AuditedWorkAuthorization {
        inner: Arc::new(UnenteredPolicy),
        associations: Vec::new().into(),
        publication: LocalAuthorizationPublication::new(),
        clock: Arc::new(FailingClock),
        sink: sink.clone(),
    };
    let context = meerkat_core::WorkAuthorizationContext::new(
        Arc::new(audited),
        OperationExecutionScope::Domain,
    );
    let candidate = OperationId::new();
    let run = meerkat_core::RunId::new();
    let withholding = meerkat_core::MandatoryHookWithholding {
        sequence: 1,
        hook_id: Some(meerkat_core::HookId::new("gate")),
        hook_id_omitted: false,
        point: meerkat_core::HookPoint::PreLlmRequest,
        cause: meerkat_core::MandatoryHookCause::LaunchRefused {
            refusal: meerkat_core::confinement::ConfinementRefusal::BackendUnavailable,
        },
        effect: meerkat_core::MandatoryHookWithheldEffect::RequestNotSent,
    };
    context
        .observe_mandatory_hook_withholding(
            candidate.clone(),
            Some(run.clone()),
            withholding.clone(),
        )
        .expect("protected custody accepts the record");
    let events = sink.0.lock().expect("observations");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].operation_id, candidate);
    assert_eq!(events[0].run_id, Some(run));
    assert_eq!(events[0].execution_scope, OperationExecutionScope::Domain);
    assert!(matches!(
        &events[0].observation,
        AuditObservation::MandatoryHookWithheld { withholding: recorded } if *recorded == withholding
    ));
    assert_eq!(
        events[0].safe_projection(),
        meerkat_authorization_contracts::audit::SafeAuditObservation::MandatoryHookWithheld
    );
}

// One bounded rebuild of the audited preparation, as the publication states
// for `observe`: "The caller may rebuild once; it must not relabel old facts
// with a newer stamp." The scripted owner publishes an owner change during the
// attempts its script names, then answers that attempt's outcome.

struct RebuildClock;
impl LocalAuthorizationClock for RebuildClock {
    fn now(&self) -> Result<crate::work::LocalAuthorizationTime, crate::clock::LocalClockError> {
        Ok(crate::work::LocalAuthorizationTime {
            unix_ms: 1,
            monotonic: meerkat_core::time_compat::Instant::now(),
        })
    }
}

struct RebuildDecision;
impl PreparedOperationAuthorization for RebuildDecision {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum RebuildOutcome {
    Prepared,
    Refused(OperationRefusalKind),
    Unavailable,
    ObservationUnavailable,
}

struct ScriptedInner {
    publication: LocalAuthorizationPublication,
    /// Per attempt: whether it publishes an owner change, then its answer.
    script: Vec<(bool, RebuildOutcome)>,
    calls: std::sync::atomic::AtomicUsize,
}

impl WorkAuthorization for ScriptedInner {
    fn prepare(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (publishes, outcome) = self
            .script
            .get(call)
            .copied()
            .expect("no attempt beyond the script");
        if publishes {
            drop(
                self.publication
                    .begin_owner_change()
                    .expect("actual publication"),
            );
        }
        match outcome {
            RebuildOutcome::Prepared => Ok(Arc::new(RebuildDecision)),
            RebuildOutcome::Refused(kind) => Err(OperationRefused::new(kind).into()),
            RebuildOutcome::Unavailable => Err(OperationAuthorizationError::Unavailable),
            RebuildOutcome::ObservationUnavailable => Err(
                OperationAuthorizationError::ObservationUnavailable(OperationObservationError),
            ),
        }
    }
}

#[derive(Default)]
struct RecordingSink(std::sync::Mutex<Vec<AuthorizationAuditObservation>>);
impl AuthorizationAuditSink for RecordingSink {
    fn append(
        &self,
        observation: AuthorizationAuditObservation,
    ) -> Result<(), OperationObservationError> {
        self.0.lock().expect("observations").push(observation);
        Ok(())
    }
}

/// The scripted preparation's result, the attempts the inner owner saw,
/// every record the sink received, the prepared operation's binding and id,
/// and the publication instance.
struct ScriptedPreparation {
    result: Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError>,
    attempts: usize,
    records: Vec<AuthorizationAuditObservation>,
    operation_id: OperationId,
    binding: PreparedAuthorizationBinding,
    sink: Arc<RecordingSink>,
    instance: uuid::Uuid,
}

fn prepare_scripted(script: Vec<(bool, RebuildOutcome)>) -> ScriptedPreparation {
    prepare_scripted_on(LocalAuthorizationPublication::new(), script)
}

fn prepare_scripted_on(
    publication: LocalAuthorizationPublication,
    script: Vec<(bool, RebuildOutcome)>,
) -> ScriptedPreparation {
    let (instance, _) = publication.current_read().expect("a live publication");
    let inner = Arc::new(ScriptedInner {
        publication: publication.clone(),
        script,
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let sink = Arc::new(RecordingSink::default());
    let work = AuditedWorkAuthorization {
        inner: inner.clone(),
        associations: Vec::new().into(),
        publication,
        clock: Arc::new(RebuildClock),
        sink: sink.clone(),
    };
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: PrincipalRef::in_domain(
                    PrincipalKind::ServiceAccount,
                    "owner",
                    TrustDomainId::new("domain").expect("domain"),
                )
                .expect("principal"),
                namespace: "fixture".into(),
                id: "fixture".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    let result = work.prepare(&binding);
    let records = sink.0.lock().expect("observations").clone();
    ScriptedPreparation {
        result,
        attempts: inner.calls.load(std::sync::atomic::Ordering::SeqCst),
        records,
        operation_id: binding.facts().operation_id.clone(),
        binding,
        sink,
        instance,
    }
}

/// The read of a scripted preparation's only `Refused` record.
fn refusal_read(prepared: &ScriptedPreparation) -> AuditRefusalRead {
    assert_eq!(prepared.records.len(), 1, "one record");
    match &prepared.records[0].observation {
        AuditObservation::Refused { read, .. } => read.clone(),
        _ => None,
    }
    .expect("a refused record with its read")
}

#[test]
fn one_owner_change_during_preparation_is_rebuilt_once_and_recorded_once() {
    let prepared = prepare_scripted(vec![
        (true, RebuildOutcome::Prepared),
        (false, RebuildOutcome::Prepared),
    ]);
    assert!(
        prepared.result.is_ok(),
        "one concurrent owner change is rebuilt from current owners"
    );
    assert_eq!(prepared.attempts, 2, "exactly one rebuild");
    assert_eq!(
        prepared.records.len(),
        1,
        "only the final attempt is recorded"
    );
    assert_eq!(prepared.records[0].operation_id, prepared.operation_id);
    assert!(
        matches!(
            &prepared.records[0].observation,
            AuditObservation::Prepared { policy, .. } if policy.publication_sequence == 2
                && policy.publication_instance == Some(prepared.instance)
        ),
        "one Prepared record carrying the rebuilt attempt's own stamp and instance"
    );
}

#[test]
fn a_second_owner_change_still_refuses_with_one_refused_record() {
    let prepared = prepare_scripted(vec![
        (true, RebuildOutcome::Prepared),
        (true, RebuildOutcome::Prepared),
    ]);
    assert!(matches!(
        prepared.result,
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired
    ));
    assert_eq!(prepared.attempts, 2, "one rebuild, then fail closed");
    assert_eq!(
        prepared.records.len(),
        1,
        "only the final attempt is recorded"
    );
    assert_eq!(prepared.records[0].operation_id, prepared.operation_id);
    assert!(matches!(
        prepared.records[0].observation,
        AuditObservation::Refused {
            reason: OperationRefusalKind::ReprepareRequired,
            ..
        }
    ));
    // The rebuilt attempt read 2 and the change it met completed at 4.
    assert_eq!(
        refusal_read(&prepared),
        AuditRefusalRead::new(
            1,
            AuditRefusalBasis::PublicationChanged,
            2,
            Some(4),
            prepared.instance,
        )
    );
}

/// An owner change still in progress refuses every attempt before the owners
/// are asked: the read names the odd sequence the attempts started from and
/// the live one, still odd.
#[test]
fn a_change_still_in_progress_reads_as_change_in_progress() {
    let publication = LocalAuthorizationPublication::new();
    let change = publication
        .begin_owner_change()
        .expect("actual publication");
    let prepared = prepare_scripted_on(publication.clone(), Vec::new());
    drop(change);
    assert!(matches!(
        prepared.result,
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired
    ));
    assert_eq!(prepared.attempts, 0, "no owner is asked during a change");
    assert_eq!(
        refusal_read(&prepared),
        AuditRefusalRead::new(
            1,
            AuditRefusalBasis::ChangeInProgress,
            1,
            Some(1),
            prepared.instance,
        )
    );
}

#[test]
fn known_infrastructure_failure_during_an_owner_change_is_not_rebuilt() {
    let unavailable = prepare_scripted(vec![(true, RebuildOutcome::Unavailable)]);
    assert!(matches!(
        unavailable.result,
        Err(OperationAuthorizationError::Unavailable)
    ));
    assert_eq!(unavailable.attempts, 1, "no retry of failed infrastructure");
    assert_eq!(unavailable.records.len(), 1);
    assert!(matches!(
        unavailable.records[0].observation,
        AuditObservation::AuthorizationUnavailable { .. }
    ));

    let unobserved = prepare_scripted(vec![(true, RebuildOutcome::ObservationUnavailable)]);
    assert!(matches!(
        unobserved.result,
        Err(OperationAuthorizationError::ObservationUnavailable(_))
    ));
    assert_eq!(unobserved.attempts, 1, "no retry of failed infrastructure");
    assert!(unobserved.records.is_empty());
}

#[test]
fn infrastructure_failure_on_the_rebuild_is_kept_and_not_retried_again() {
    let unavailable = prepare_scripted(vec![
        (true, RebuildOutcome::Prepared),
        (false, RebuildOutcome::Unavailable),
    ]);
    assert!(matches!(
        unavailable.result,
        Err(OperationAuthorizationError::Unavailable)
    ));
    assert_eq!(unavailable.attempts, 2);
    assert_eq!(unavailable.records.len(), 1);
    assert!(matches!(
        unavailable.records[0].observation,
        AuditObservation::AuthorizationUnavailable { .. }
    ));

    let unobserved = prepare_scripted(vec![
        (true, RebuildOutcome::Prepared),
        (false, RebuildOutcome::ObservationUnavailable),
    ]);
    assert!(matches!(
        unobserved.result,
        Err(OperationAuthorizationError::ObservationUnavailable(_))
    ));
    assert_eq!(unobserved.attempts, 2);
    assert!(unobserved.records.is_empty());
}

#[test]
fn a_coherent_policy_refusal_is_final_and_not_rebuilt() {
    for kind in [
        OperationRefusalKind::Denied,
        OperationRefusalKind::MalformedFacts,
    ] {
        let refused = prepare_scripted(vec![(false, RebuildOutcome::Refused(kind))]);
        assert!(matches!(
            refused.result,
            Err(OperationAuthorizationError::Refused(refusal)) if refusal.kind() == kind
        ));
        assert_eq!(refused.attempts, 1, "no retry of a coherent refusal");
        assert_eq!(refused.records.len(), 1);
        assert!(matches!(
            refused.records[0].observation,
            AuditObservation::Refused { reason, .. } if reason == kind
        ));
        assert!(
            refusal_read(&refused)
                == AuditRefusalRead::new(
                    1,
                    AuditRefusalBasis::OwnerAnswer,
                    0,
                    None,
                    refused.instance,
                ),
            "the owners answered at the coherent read"
        );
    }
}

/// A refusal the prepared decision observes later, at its check, is the
/// owners' answer at the decision's own coherent read.
#[test]
fn a_check_time_refusal_reads_at_the_decision_own_read() {
    let prepared = prepare_scripted(vec![(false, RebuildOutcome::Prepared)]);
    let decision = prepared.result.as_ref().expect("prepared");
    decision
        .observe(
            &prepared.binding,
            OperationObservation::Refused(OperationRefusalKind::Denied),
        )
        .expect("observed");
    let records = prepared.sink.0.lock().expect("observations").clone();
    assert_eq!(records.len(), 2, "Prepared, then the check-time refusal");
    assert!(matches!(
        &records[1].observation,
        AuditObservation::Refused {
            reason: OperationRefusalKind::Denied,
            read: Some(read),
            ..
        } if *read
            == AuditRefusalRead::new(
                1,
                AuditRefusalBasis::OwnerAnswer,
                0,
                None,
                prepared.instance,
            )
    ));
}

/// A refusal to rebuild the prepared decision observes at its check means
/// the publication moved after the decision's own read (an observation of
/// another binding is refused before it is recorded), so it reads as
/// changed against the live sequence, as at prepare time: first with the
/// change still in progress, then once it completed.
#[test]
fn a_check_time_refusal_to_rebuild_reads_as_the_publication_changed() {
    let publication = LocalAuthorizationPublication::new();
    let prepared =
        prepare_scripted_on(publication.clone(), vec![(false, RebuildOutcome::Prepared)]);
    let decision = prepared.result.as_ref().expect("prepared");
    let refuse_to_rebuild = || {
        decision
            .observe(
                &prepared.binding,
                OperationObservation::Refused(OperationRefusalKind::ReprepareRequired),
            )
            .expect("observed");
    };
    let change = publication.begin_owner_change().expect("owner change");
    refuse_to_rebuild();
    drop(change);
    refuse_to_rebuild();
    let records = prepared.sink.0.lock().expect("observations").clone();
    assert_eq!(records.len(), 3, "Prepared, then two check-time refusals");
    let reads = records[1..]
        .iter()
        .map(|record| match &record.observation {
            AuditObservation::Refused {
                reason: OperationRefusalKind::ReprepareRequired,
                read,
                ..
            } => read.clone(),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        reads
            == vec![
                Some(AuditRefusalRead::new(
                    1,
                    AuditRefusalBasis::ChangeInProgress,
                    0,
                    Some(1),
                    prepared.instance,
                )),
                Some(AuditRefusalRead::new(
                    1,
                    AuditRefusalBasis::PublicationChanged,
                    0,
                    Some(2),
                    prepared.instance,
                )),
            ],
        "a change in progress, then a completed change"
    );
}

#[test]
fn the_rebuild_answers_from_current_owners_not_from_the_changed_read() {
    // A revocation published during the first read is honored.
    let revoked = prepare_scripted(vec![
        (true, RebuildOutcome::Prepared),
        (false, RebuildOutcome::Refused(OperationRefusalKind::Denied)),
    ]);
    assert!(
        matches!(
            revoked.result,
            Err(OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "the rebuilt read refuses"
    );
    assert_eq!(revoked.attempts, 2);
    assert_eq!(revoked.records.len(), 1);
    assert!(matches!(
        revoked.records[0].observation,
        AuditObservation::Refused {
            reason: OperationRefusalKind::Denied,
            ..
        }
    ));

    // A refusal read during an owner change is no verdict; the current owners
    // answer.
    let granted = prepare_scripted(vec![
        (true, RebuildOutcome::Refused(OperationRefusalKind::Denied)),
        (false, RebuildOutcome::Prepared),
    ]);
    assert!(granted.result.is_ok());
    assert_eq!(granted.attempts, 2);
    assert_eq!(granted.records.len(), 1);
    assert!(matches!(
        &granted.records[0].observation,
        AuditObservation::Prepared { policy, .. } if policy.publication_sequence == 2
    ));
}

/// A hosted-search delegate's protected records name its parent operation
/// and role, projected from its binding; the parent's own records carry no
/// delegate attribution. The projection is protected history only.
#[tokio::test]
async fn a_delegate_observation_names_its_parent_operation() {
    use meerkat_authorization_contracts::audit::{AuditDelegateAttribution, AuditDelegateRole};
    use meerkat_core::authorization::{ModelAuthorizationFacts, ToolDelegateRole};
    struct Allowed;
    impl PreparedOperationAuthorization for Allowed {
        fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
            meerkat_core::authorization::OperationReviewTier::R1
        }

        fn check_current(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<(), OperationAuthorizationError> {
            Ok(())
        }
    }
    struct AllowAll;
    impl WorkAuthorization for AllowAll {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Ok(Arc::new(Allowed))
        }
    }
    struct FixedClock;
    impl LocalAuthorizationClock for FixedClock {
        fn now(
            &self,
        ) -> Result<crate::work::LocalAuthorizationTime, crate::clock::LocalClockError> {
            Ok(crate::work::LocalAuthorizationTime {
                unix_ms: 100,
                monotonic: meerkat_core::time_compat::Instant::now(),
            })
        }
    }
    #[derive(Default)]
    struct Sink(std::sync::Mutex<Vec<AuthorizationAuditObservation>>);
    impl AuthorizationAuditSink for Sink {
        fn append(
            &self,
            observation: AuthorizationAuditObservation,
        ) -> Result<(), OperationObservationError> {
            self.0.lock().expect("observations").push(observation);
            Ok(())
        }
    }
    let sink = Arc::new(Sink::default());
    let work = meerkat_core::WorkAuthorizationContext::new(
        Arc::new(AuditedWorkAuthorization {
            inner: Arc::new(AllowAll),
            associations: Vec::new().into(),
            publication: LocalAuthorizationPublication::new(),
            clock: Arc::new(FixedClock),
            sink: sink.clone(),
        }),
        OperationExecutionScope::Domain,
    );
    let attribution = meerkat_core::authorization::__test_mint_tool_delegate(
        &work,
        None,
        ToolDelegateRole::HostedSearchInference,
    )
    .await
    .expect("the governed builtin call mints its delegate");
    let parent = attribution.parent_binding().facts().operation_id.clone();
    let helper = ModelAuthorizationFacts {
        identity: Arc::new(meerkat_core::SessionLlmIdentity {
            model: "search-model".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: "search-wire-model".into(),
        hosted_capabilities: Arc::from([meerkat_core::ServerToolKind::WebSearch]),
        backend_profile_id: Some("search-profile".into()),
        backend_kind: "openai".into(),
        endpoint: "https://search.invalid/responses".into(),
        credential: None,
        usage: ModelAuthorizationUse::Inference,
        live_channel: None,
    };
    let prepared = meerkat_core::LlmRequestAuthorization::for_tool_delegate(attribution)
        .prepare(helper)
        .expect("the delegate is prepared");
    prepared
        .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
        .expect("entry staged");
    let delegate = prepared.binding().facts().operation_id.clone();
    let records = sink.0.lock().expect("observations").clone();
    let expected = AuditDelegateAttribution {
        parent_operation_id: parent.clone(),
        role: AuditDelegateRole::HostedSearchInference,
    };
    let helper_records: Vec<_> = records
        .iter()
        .filter(|record| record.operation_id == delegate)
        .collect();
    assert_eq!(helper_records.len(), 2, "prepared and entry");
    assert!(matches!(
        helper_records[0].observation,
        AuditObservation::Prepared { .. }
    ));
    assert!(matches!(
        helper_records[1].observation,
        AuditObservation::Entry {
            boundary: Some(meerkat_core::OperationEntryBoundary::ModelRequest)
        }
    ));
    for record in &helper_records {
        assert_eq!(record.delegate_attribution.as_ref(), Some(&expected));
        assert!(record.review_attribution.is_none());
    }
    let parent_records: Vec<_> = records
        .iter()
        .filter(|record| record.operation_id == parent)
        .collect();
    assert!(!parent_records.is_empty(), "the parent call's own records");
    assert!(
        parent_records
            .iter()
            .all(|record| record.delegate_attribution.is_none()),
        "the parent is not a delegate"
    );
    let encoded = serde_json::to_value(helper_records[0]).expect("protected record");
    assert_eq!(
        encoded["delegate_attribution"]["role"],
        serde_json::json!("hosted_search_inference")
    );
    assert_eq!(
        serde_json::from_value::<AuthorizationAuditObservation>(encoded).expect("round trip"),
        *helper_records[0]
    );
    assert_eq!(
        serde_json::to_value(helper_records[0].safe_projection()).expect("safe projection"),
        serde_json::json!("prepared")
    );
}
