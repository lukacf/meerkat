#![allow(clippy::expect_used, clippy::panic)]

use meerkat_jobs::{
    PredicateComparison, PredicateEvaluation, PredicateObservation, PredicatePollingPolicy,
    PredicateSource, PredicateWatch, PredicateWatchError, PredicateWatchId, RestartClass,
    ScheduleIdRef,
};

fn policy() -> PredicatePollingPolicy {
    PredicatePollingPolicy::new(60, 4, 10, 2, 300).expect("policy")
}

#[test]
fn poll_watches_require_schedule_authority_and_enforce_steady_state_bounds() {
    let error = PredicatePollingPolicy::new(5, 4, 10, 2, 300)
        .expect_err("polling below the global minimum fails closed");
    assert!(matches!(
        error,
        PredicateWatchError::PollingIntervalTooShort {
            requested_secs: 5,
            minimum_secs: 30
        }
    ));

    let watch = PredicateWatch::scheduled(
        PredicateWatchId::new("release-watch").expect("watch id"),
        ScheduleIdRef::new("schedule-release-watch").expect("schedule id"),
        PredicateSource::StableHttp {
            url: "https://example.invalid/releases/latest".into(),
            conditional_requests: true,
        },
        PredicateComparison::Changed,
        policy(),
    )
    .expect("watch");
    assert_eq!(watch.restart_class(), RestartClass::Replayable);
    assert_eq!(watch.schedule_id().as_str(), "schedule-release-watch");
    assert_eq!(watch.polling_policy().max_concurrency(), 4);
    assert_eq!(watch.polling_policy().jitter_percent(), 10);
    assert_eq!(watch.polling_policy().max_backoff_secs(), 300);

    assert!(matches!(
        PredicatePollingPolicy::new(60, 65, 0, 1, 300),
        Err(PredicateWatchError::InvalidPollingPolicy(_))
    ));
    assert!(matches!(
        PredicatePollingPolicy::new(60, 1, 0, 10_001, 300),
        Err(PredicateWatchError::InvalidPollingPolicy(_))
    ));
}

#[test]
fn source_restart_class_is_derived_from_real_dependency_durability() {
    assert_eq!(
        PredicateSource::PersistentFile {
            path: "/durable/state/version".into()
        }
        .restart_class(),
        RestartClass::CheckpointResumable
    );
    assert_eq!(
        PredicateSource::DurablePush {
            adapter: "github-webhook".into(),
            cursor_name: "delivery-id".into(),
        }
        .restart_class(),
        RestartClass::Adoptable
    );
    assert_eq!(
        PredicateSource::HostDependent {
            adapter: "homecore-python".into()
        }
        .restart_class(),
        RestartClass::NonResumable
    );
}

#[test]
fn baseline_is_record_only_and_two_later_distinct_observations_notify_once_each() {
    let watch = PredicateWatch::scheduled(
        PredicateWatchId::new("version-watch").expect("watch id"),
        ScheduleIdRef::new("schedule-version").expect("schedule id"),
        PredicateSource::StableHttp {
            url: "https://example.invalid/version".into(),
            conditional_requests: true,
        },
        PredicateComparison::Changed,
        policy(),
    )
    .expect("watch");

    let baseline = watch
        .evaluate(
            None,
            PredicateObservation::available("v1", "Version v1").expect("observation"),
        )
        .expect("baseline");
    assert!(matches!(baseline, PredicateEvaluation::Baseline { .. }));
    let checkpoint = baseline.checkpoint().expect("checkpoint").clone();

    let replay = watch
        .evaluate(
            Some(&checkpoint),
            PredicateObservation::available("v1", "Version v1").expect("observation"),
        )
        .expect("replay");
    assert!(matches!(replay, PredicateEvaluation::Unchanged { .. }));

    let second = watch
        .evaluate(
            Some(&checkpoint),
            PredicateObservation::available("v2", "Version v2").expect("observation"),
        )
        .expect("second");
    let PredicateEvaluation::Crossed {
        checkpoint,
        notification,
    } = second
    else {
        panic!("expected crossing");
    };
    assert_eq!(notification.idempotency_key(), "watch:version-watch:v2");

    let third = watch
        .evaluate(
            Some(&checkpoint),
            PredicateObservation::available("v3", "Version v3").expect("observation"),
        )
        .expect("third");
    let PredicateEvaluation::Crossed { notification, .. } = third else {
        panic!("expected crossing");
    };
    assert_eq!(notification.idempotency_key(), "watch:version-watch:v3");
}

#[test]
fn unavailable_host_source_is_typed_retryable_and_never_fabricates_notification() {
    let watch = PredicateWatch::scheduled(
        PredicateWatchId::new("host-watch").expect("watch id"),
        ScheduleIdRef::new("schedule-host").expect("schedule id"),
        PredicateSource::HostDependent {
            adapter: "homecore-python".into(),
        },
        PredicateComparison::Changed,
        policy(),
    )
    .expect("watch");
    let evaluation = watch
        .evaluate(
            None,
            PredicateObservation::unavailable("host_offline").expect("unavailable"),
        )
        .expect("evaluation");
    assert!(matches!(
        evaluation,
        PredicateEvaluation::SourceUnavailable {
            ref reason,
            retry_after_secs: 60,
            ..
        } if reason == "host_offline"
    ));
    assert!(evaluation.notification().is_none());
}

#[test]
fn predicate_observations_and_notifications_reject_unbounded_payloads() {
    assert!(matches!(
        PredicateObservation::available("k", "x".repeat(64 * 1024 + 1)),
        Err(PredicateWatchError::InvalidComponent { .. })
    ));
    assert!(
        meerkat_jobs::JobNotification::new(
            "notification",
            "key",
            "title",
            "x".repeat(64 * 1024 + 1),
        )
        .is_err()
    );
}
