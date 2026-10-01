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
            | AuditObservation::Entry
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
        let (_, before) = publication.observe(|| ()).expect("initial stamp");
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
        let (_, before) = publication.observe(|| ()).expect("initial stamp");
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
