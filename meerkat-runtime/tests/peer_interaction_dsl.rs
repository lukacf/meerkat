//! W1-A contract tests for the peer-interaction DSL lifecycle.
//!
//! Each test exercises a single typed DSL transition on an ephemeral
//! `RuntimePeerInteractionHandle` sharing a DSL authority bootstrapped to
//! the `Idle` phase (Initialize + RegisterSession). Verifies that
//! `outbound_state` / `inbound_state` advance as the transition describes
//! and (for terminal-shaped transitions) that the projection-cleanup
//! invariant holds — the map entry is removed so downstream subscriber /
//! stream channels can be dropped deterministically.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::handles::{PeerInteractionHandle, PeerTerminalDisposition};
use meerkat_core::{InboundPeerRequestState, OutboundPeerRequestState, PeerCorrelationId};
use meerkat_runtime::handles::{HandleDslAuthority, RuntimePeerInteractionHandle};
use meerkat_runtime::meerkat_machine::dsl as mm_dsl;

/// Build an ephemeral peer-interaction handle with the DSL authority driven
/// into the Idle phase via `Initialize` + `RegisterSession`. Without this
/// bootstrap the authority sits in `Initializing`, where peer-interaction
/// transitions do not match.
fn new_handle() -> RuntimePeerInteractionHandle {
    let dsl = Arc::new(HandleDslAuthority::ephemeral());
    dsl.apply_signal(mm_dsl::MeerkatMachineSignal::Initialize, "test::initialize")
        .expect("Initialize signal");
    dsl.apply_input(
        mm_dsl::MeerkatMachineInput::RegisterSession {
            session_id: mm_dsl::SessionId::from("peer-interaction-test".to_string()),
        },
        "test::register_session",
    )
    .expect("RegisterSession input");
    RuntimePeerInteractionHandle::new(dsl)
}

#[test]
fn request_sent_advances_pending_map_to_sent() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    assert!(handle.outbound_state(corr_id).is_none());
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    assert_eq!(
        handle.outbound_state(corr_id),
        Some(OutboundPeerRequestState::Sent)
    );
}

#[test]
fn request_sent_rejects_duplicate() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    let err = handle
        .request_sent(corr_id, "peer-a".into())
        .expect_err("duplicate send must reject");
    assert_eq!(err.context, "PeerInteractionHandle::request_sent");
}

#[test]
fn progress_advances_state_to_accepted() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    handle.response_progress(corr_id).unwrap();
    assert_eq!(
        handle.outbound_state(corr_id),
        Some(OutboundPeerRequestState::AcceptedProgress)
    );
}

#[test]
fn progress_rejects_unknown_corr_id() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    let err = handle
        .response_progress(corr_id)
        .expect_err("progress on unknown corr_id must reject");
    assert_eq!(err.context, "PeerInteractionHandle::response_progress");
}

#[test]
fn terminal_completed_removes_entry_and_emits_cleanup() {
    // Load-bearing proof that `pending_peer_requests` is a real projection:
    // terminal transition removes the entry so any channel keyed on the same
    // corr_id can be dropped by the shell-side cleanup observer.
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    handle
        .response_terminal(corr_id, PeerTerminalDisposition::Completed)
        .unwrap();
    // Map entry gone — second terminal is rejected by the `pending_exists`
    // guard (structural shape of the cleanup effect).
    assert!(handle.outbound_state(corr_id).is_none());
    let err = handle
        .response_terminal(corr_id, PeerTerminalDisposition::Completed)
        .expect_err("second terminal must reject");
    assert_eq!(err.context, "PeerInteractionHandle::response_terminal");
}

#[test]
fn terminal_failed_removes_entry() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    handle
        .response_terminal(corr_id, PeerTerminalDisposition::Failed)
        .unwrap();
    assert!(handle.outbound_state(corr_id).is_none());
}

#[test]
fn timeout_removes_entry_and_emits_cleanup() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    handle.request_timed_out(corr_id).unwrap();
    assert!(handle.outbound_state(corr_id).is_none());
    // Second timeout rejects — cleanup is exactly once.
    let err = handle
        .request_timed_out(corr_id)
        .expect_err("second timeout must reject");
    assert_eq!(err.context, "PeerInteractionHandle::request_timed_out");
}

#[test]
fn inbound_received_then_replied_advances_and_removes() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    assert!(handle.inbound_state(corr_id).is_none());
    handle.request_received(corr_id).unwrap();
    assert_eq!(
        handle.inbound_state(corr_id),
        Some(InboundPeerRequestState::Received)
    );
    handle.response_replied(corr_id).unwrap();
    assert!(handle.inbound_state(corr_id).is_none());
}

#[test]
fn inbound_received_rejects_duplicate() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_received(corr_id).unwrap();
    let err = handle
        .request_received(corr_id)
        .expect_err("duplicate inbound receipt must reject");
    assert_eq!(err.context, "PeerInteractionHandle::request_received");
}

#[test]
fn inbound_replied_rejects_unknown_corr_id() {
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    let err = handle
        .response_replied(corr_id)
        .expect_err("reply on unknown corr_id must reject");
    assert_eq!(err.context, "PeerInteractionHandle::response_replied");
}

#[test]
fn cleanup_observer_fires_on_terminal_transitions() {
    use meerkat_core::handles::PeerInteractionCleanupObserver;
    use std::sync::Mutex;

    struct Recorder(Mutex<Vec<PeerCorrelationId>>);
    impl PeerInteractionCleanupObserver for Recorder {
        fn on_peer_interaction_cleanup(&self, corr_id: PeerCorrelationId) {
            self.0.lock().unwrap().push(corr_id);
        }
    }

    let handle = new_handle();
    let rec = Arc::new(Recorder(Mutex::new(Vec::new())));
    handle.install_cleanup_observer(Arc::clone(&rec) as Arc<dyn PeerInteractionCleanupObserver>);

    // Sent / Progress do NOT fire cleanup.
    let a = PeerCorrelationId::new();
    handle.request_sent(a, "peer-a".into()).unwrap();
    handle.response_progress(a).unwrap();
    assert!(
        rec.0.lock().unwrap().is_empty(),
        "non-terminal transitions must not fire cleanup"
    );

    // Terminal Completed fires cleanup once.
    handle
        .response_terminal(a, PeerTerminalDisposition::Completed)
        .unwrap();
    assert_eq!(rec.0.lock().unwrap().clone(), vec![a]);

    // Terminal Failed fires cleanup once.
    let b = PeerCorrelationId::new();
    handle.request_sent(b, "peer-b".into()).unwrap();
    handle
        .response_terminal(b, PeerTerminalDisposition::Failed)
        .unwrap();
    assert_eq!(rec.0.lock().unwrap().clone(), vec![a, b]);

    // TimedOut fires cleanup once.
    let c = PeerCorrelationId::new();
    handle.request_sent(c, "peer-c".into()).unwrap();
    handle.request_timed_out(c).unwrap();
    assert_eq!(rec.0.lock().unwrap().clone(), vec![a, b, c]);

    // Inbound reply does NOT fire `PeerInteractionCleanup` (it emits the
    // inbound state-change effect on a different variant).
    let d = PeerCorrelationId::new();
    handle.request_received(d).unwrap();
    handle.response_replied(d).unwrap();
    assert_eq!(
        rec.0.lock().unwrap().clone(),
        vec![a, b, c],
        "inbound lifecycle does not emit PeerInteractionCleanup"
    );
}

#[test]
fn cleanup_observer_is_weakly_held_no_arc_cycle() {
    // P1 regression guard: the runtime impl stores the cleanup observer as
    // a `Weak`, so installing it does not keep the owner alive. Drop the
    // strong `Arc` and confirm subsequent cleanup fires no longer reach the
    // observer — i.e., the handle held the Weak, the strong count dropped
    // to zero with the `Arc`, and the handle's `upgrade()` now returns
    // `None`. Without the fix this test would either keep the recorder
    // alive (and see new records) or, in the production cycle, leak the
    // `CommsRuntime`.
    use meerkat_core::handles::PeerInteractionCleanupObserver;
    use std::sync::Mutex;

    struct Recorder(Mutex<Vec<PeerCorrelationId>>);
    impl PeerInteractionCleanupObserver for Recorder {
        fn on_peer_interaction_cleanup(&self, corr_id: PeerCorrelationId) {
            self.0.lock().unwrap().push(corr_id);
        }
    }

    let handle = new_handle();
    let rec: Arc<Recorder> = Arc::new(Recorder(Mutex::new(Vec::new())));
    let rec_weak = Arc::downgrade(&rec);
    handle.install_cleanup_observer(Arc::clone(&rec) as Arc<dyn PeerInteractionCleanupObserver>);
    assert_eq!(
        Arc::strong_count(&rec),
        1,
        "handle must not hold a strong ref"
    );

    // Drop the caller's last strong ref; observer should vanish.
    drop(rec);
    assert!(
        rec_weak.upgrade().is_none(),
        "observer must be fully dropped once the caller releases its Arc — no cycle"
    );

    // A terminal transition after observer drop must still succeed on the
    // DSL side and must not panic in the dispatch path when the weak fails
    // to upgrade.
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    handle
        .response_terminal(corr_id, PeerTerminalDisposition::Completed)
        .expect("terminal transition must succeed with dropped observer");
    assert!(handle.outbound_state(corr_id).is_none());
}

#[test]
fn outbound_inbound_are_independent_namespaces() {
    // Mixing outbound and inbound on the same corr_id must not alias.
    let handle = new_handle();
    let corr_id = PeerCorrelationId::new();
    handle.request_sent(corr_id, "peer-a".into()).unwrap();
    handle.request_received(corr_id).unwrap();
    assert_eq!(
        handle.outbound_state(corr_id),
        Some(OutboundPeerRequestState::Sent)
    );
    assert_eq!(
        handle.inbound_state(corr_id),
        Some(InboundPeerRequestState::Received)
    );
    handle
        .response_terminal(corr_id, PeerTerminalDisposition::Completed)
        .unwrap();
    // Inbound entry is untouched by outbound terminal.
    assert!(handle.outbound_state(corr_id).is_none());
    assert_eq!(
        handle.inbound_state(corr_id),
        Some(InboundPeerRequestState::Received)
    );
}
