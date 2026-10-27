//! Deterministic fault points for process-level acceptance tests.
//!
//! These compile to nothing unless the `announcement-test-faults` feature is
//! on. With it, `META_ANNOUNCEMENT_FAULT=<point>` makes the process abort at
//! that point, so a test can kill a gateway between two durable commits
//! without racing it. No production build carries them.

#[cfg(feature = "announcement-test-faults")]
fn hit(point: &str) {
    if std::env::var("META_ANNOUNCEMENT_FAULT").is_ok_and(|value| value == point) {
        eprintln!("announcement fault point {point}: aborting the process");
        std::process::abort();
    }
}

/// After the native owner committed a batch, before the announcement owner
/// records the receipt: the acknowledgment is lost.
pub(crate) fn after_native_commit() {
    #[cfg(feature = "announcement-test-faults")]
    hit("after_native_commit");
}

/// After the intent to submit is durable, before any native submission.
pub(crate) fn after_begin_submit() {
    #[cfg(feature = "announcement-test-faults")]
    hit("after_begin_submit");
}

/// On a gateway serving a peer's keyed inject: after its continuation owner
/// committed the delivery, before the signed receipt is answered. The caller
/// never learns the outcome from this attempt.
pub(crate) fn after_remote_commit() {
    #[cfg(feature = "announcement-test-faults")]
    hit("after_remote_commit");
}

/// Whether `META_ANNOUNCEMENT_UNOBSERVED_OWNER` names `session_id`: the
/// session-service decorator then answers that the session's durable source
/// cannot be observed, as for an owner a restart cannot observe yet, so
/// Meerkat restores a child mob that session owns held until its owner
/// resolves. Always `false` without the feature.
pub(crate) fn owner_unobserved(session_id: &meerkat_core::SessionId) -> bool {
    #[cfg(feature = "announcement-test-faults")]
    {
        std::env::var("META_ANNOUNCEMENT_UNOBSERVED_OWNER")
            .ok()
            .and_then(|value| meerkat_core::SessionId::parse(value.trim()).ok())
            .is_some_and(|named| &named == session_id)
    }
    #[cfg(not(feature = "announcement-test-faults"))]
    {
        let _ = session_id;
        false
    }
}

/// Inside a source advance's transaction, after its items, its cursor and
/// its receipt were written and before the commit.
pub(crate) fn inside_source_advance() {
    #[cfg(feature = "announcement-test-faults")]
    hit("inside_source_advance");
}

/// After a source advance committed, before its answer.
pub(crate) fn after_source_advance() {
    #[cfg(feature = "announcement-test-faults")]
    hit("after_source_advance");
}
