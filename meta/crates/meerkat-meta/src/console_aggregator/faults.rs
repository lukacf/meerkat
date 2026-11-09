//! Deterministic fault points for the console send process tests.
//!
//! These compile to nothing unless the `console-test-faults` feature is on.
//! With it, `META_CONSOLE_FAULT=<point>` makes the process abort at that
//! point, so a test can stop a gateway between a send's durable acceptance
//! and its native admission (or between admission and settlement) without
//! racing it. No production build carries them.

#[cfg(feature = "console-test-faults")]
fn hit(point: &str) {
    if std::env::var("META_CONSOLE_FAULT").is_ok_and(|value| value == point) {
        eprintln!("console fault point {point}: aborting the process");
        std::process::abort();
    }
}

/// After a console send's frame is durably accepted, before anything is
/// submitted for native admission.
pub(crate) fn after_console_reserve() {
    #[cfg(feature = "console-test-faults")]
    hit("after_console_reserve");
}

/// After native admission acknowledged a console send, before its frame
/// records the delivery.
pub(crate) fn after_console_admission() {
    #[cfg(feature = "console-test-faults")]
    hit("after_console_admission");
}
