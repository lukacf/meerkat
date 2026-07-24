#![allow(clippy::expect_used)]

use meerkat_tools::builtin::shell::{
    MonitorAction, MonitorLineOutcome, MonitorOutputProtocol, MonitorProtocolDecoder,
    MonitorProtocolError, MonitorProtocolLimits, MonitorSuppressionReason,
};

fn limits() -> MonitorProtocolLimits {
    MonitorProtocolLimits {
        max_line_bytes: 256,
        max_notifications_per_window: 2,
        notification_window_ms: 1_000,
        max_retained_diagnostic_bytes: 32,
    }
}

#[test]
fn framed_jsonl_distinguishes_notify_checkpoint_progress_and_complete() {
    let mut decoder =
        MonitorProtocolDecoder::new(MonitorOutputProtocol::FramedJsonl, limits()).expect("decoder");

    assert_eq!(
        decoder
            .decode_stdout_line_at(
                r#"{"type":"notify","key":"release:v1","title":"Release","message":"v1 is live"}"#,
                1,
            )
            .expect("notify"),
        MonitorLineOutcome::Action(MonitorAction::Notify {
            key: "release:v1".into(),
            title: "Release".into(),
            message: "v1 is live".into(),
        })
    );
    assert_eq!(
        decoder
            .decode_stdout_line_at(r#"{"type":"checkpoint","value":"etag:abc"}"#, 2,)
            .expect("checkpoint"),
        MonitorLineOutcome::Action(MonitorAction::Checkpoint {
            value: "etag:abc".into()
        })
    );
    assert_eq!(
        decoder
            .decode_stdout_line_at(r#"{"type":"progress","cursor":7,"message":"poll 7"}"#, 3,)
            .expect("progress"),
        MonitorLineOutcome::Action(MonitorAction::Progress {
            cursor: 7,
            message: "poll 7".into(),
        })
    );
    assert_eq!(
        decoder
            .decode_stdout_line_at(r#"{"type":"complete"}"#, 4)
            .expect("complete"),
        MonitorLineOutcome::Action(MonitorAction::Complete)
    );
    assert_eq!(
        decoder
            .decode_stdout_line_at("ordinary diagnostic output", 5)
            .expect("diagnostic"),
        MonitorLineOutcome::Diagnostic
    );
}

#[test]
fn line_mode_is_explicit_and_each_complete_line_is_a_notification() {
    let mut decoder =
        MonitorProtocolDecoder::new(MonitorOutputProtocol::Lines, limits()).expect("decoder");
    assert_eq!(
        decoder
            .decode_stdout_line_at("release v2", 10)
            .expect("line"),
        MonitorLineOutcome::Action(MonitorAction::Notify {
            key: "line:1".into(),
            title: "Monitor notification".into(),
            message: "release v2".into(),
        })
    );
}

#[test]
fn malformed_oversized_and_flood_outcomes_are_typed_observable_and_recoverable() {
    let mut decoder =
        MonitorProtocolDecoder::new(MonitorOutputProtocol::FramedJsonl, limits()).expect("decoder");
    assert!(matches!(
        decoder.decode_stdout_line_at(r#"{"type":"notify"}"#, 1),
        Err(MonitorProtocolError::MalformedFrame(_))
    ));
    assert!(matches!(
        decoder.decode_stdout_line_at(&"x".repeat(257), 1),
        Err(MonitorProtocolError::LineTooLong {
            actual: 257,
            limit: 256
        })
    ));

    for (key, at) in [("a", 10), ("b", 11)] {
        decoder
            .decode_stdout_line_at(
                &format!(r#"{{"type":"notify","key":"{key}","message":"{key}"}}"#),
                at,
            )
            .expect("within budget");
    }
    assert_eq!(
        decoder
            .decode_stdout_line_at(r#"{"type":"notify","key":"c","message":"c"}"#, 12,)
            .expect("suppressed"),
        MonitorLineOutcome::Suppressed {
            reason: MonitorSuppressionReason::NotificationRateLimited,
            total_suppressed: 1,
        }
    );
    assert!(decoder.health().rate_limited);
    assert_eq!(decoder.health().suppressed_notifications, 1);

    assert!(matches!(
        decoder
            .decode_stdout_line_at(
                r#"{"type":"notify","key":"d","message":"d"}"#,
                1_011,
            )
            .expect("window recovered"),
        MonitorLineOutcome::Action(MonitorAction::Notify { key, .. }) if key == "d"
    ));
    assert!(!decoder.health().rate_limited);
}

#[test]
fn caller_overrides_cannot_disable_global_monitor_resource_bounds() {
    let defaults = MonitorProtocolLimits::default();
    for limits in [
        MonitorProtocolLimits {
            max_line_bytes: 1024 * 1024 + 1,
            ..defaults
        },
        MonitorProtocolLimits {
            max_notifications_per_window: 1_001,
            ..defaults
        },
        MonitorProtocolLimits {
            notification_window_ms: 999,
            ..defaults
        },
        MonitorProtocolLimits {
            max_retained_diagnostic_bytes: 4 * 1024 * 1024 + 1,
            ..defaults
        },
    ] {
        assert!(matches!(
            MonitorProtocolDecoder::new(MonitorOutputProtocol::FramedJsonl, limits),
            Err(MonitorProtocolError::InvalidLimits(_))
        ));
    }
}

#[test]
fn retained_diagnostics_are_tail_bounded() {
    let mut decoder =
        MonitorProtocolDecoder::new(MonitorOutputProtocol::FramedJsonl, limits()).expect("decoder");
    decoder
        .decode_stdout_line_at("first diagnostic is rather long", 1)
        .expect("first");
    decoder
        .decode_stderr_line("second diagnostic is also long")
        .expect("stderr");
    assert!(decoder.retained_diagnostics().len() <= 32);
    assert!(decoder.health().diagnostic_bytes_dropped > 0);
}
