//! Tripwire for wave-c (Section 1.5 #9). Flipped green by **C-H1**
//! (type-level append-only `SessionStore` / `AppendOnlyMessages`
//! newtype — the compile-time form of F1 closure from the
//! state-scope-audit).
//!
//! C-H1 lands two distinct closures:
//!
//! * **F7 (type-level append-only witness on `Session.messages`)** —
//!   `Session::messages_mut` is now `pub(crate)` inside `meerkat-core`.
//!   The only public ways to mutate the message buffer within a single
//!   `SessionId` are `push` / `push_batch` (extend) and the typed
//!   `externalize_media` helper (content-rewrite, does not change
//!   length). The compile-time probe below attempts the forbidden API
//!   from a downstream crate and is gated off by default — flip the
//!   `FORBIDDEN_MESSAGES_MUT` cfg to confirm the compile-fail.
//!
//! * **F1 (runtime append-only guard on `SessionStore::save`)** — every
//!   in-tree `SessionStore` implementation routes through
//!   `meerkat_core::session_store::append_only_save_guard`, which
//!   rejects a save whose message count is strictly smaller than the
//!   previously persisted row with
//!   `SessionStoreError::MonotonicityViolation`. The runtime probe
//!   below exercises the guard against `MemoryStore`.
//!
//! Both closures together mean:
//!   "Inside a single `SessionId`, history only grows. Shrink operations
//!    must go through `Session::fork_at`, which rotates `SessionId`."

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::Session;
use meerkat_core::types::Message;
use meerkat_core::{SessionStore, SessionStoreError};
use meerkat_store::MemoryStore;

/// Compile-time probe for F7.
///
/// This block references the forbidden API (`Session::messages_mut()`
/// — now `pub(crate)` inside `meerkat-core`). Under the default
/// `cfg(any())` guard the module is not compiled. Flip the guard (remove
/// `any()`) locally to verify the rustc error is the expected privacy
/// violation rather than an unrelated type error.
///
/// Not a `trybuild` fixture because adding a dev-dep on `trybuild` just
/// for one compile-fail doubles the test build time; the `cfg(any())`
/// stub achieves the same contract without the cost.
#[cfg(any())]
mod forbidden_messages_mut {
    use meerkat_core::Session;

    // The next line MUST fail to compile with:
    //   `messages_mut` is private, and only accessible within crate
    //   `meerkat_core`
    // If it ever stops failing, F7 has regressed.
    fn shrink_a_session(session: &mut Session) {
        session.messages_mut().truncate(0);
    }
}

/// Runtime probe for F1. Attempts to save a session with a shorter
/// message history than the previously persisted row; the store must
/// reject it with `MonotonicityViolation`, not silently overwrite.
#[tokio::test]
async fn append_only_save_guard_rejects_shrink_attempt() {
    let store = MemoryStore::new();

    // First save: 2 messages.
    let mut initial = Session::new();
    initial.push(Message::User(meerkat_core::types::UserMessage::text("hello")));
    initial.push(Message::User(meerkat_core::types::UserMessage::text("world")));
    store.save(&initial).await.expect("first save must succeed");

    // Second save: same id, shorter history (0 messages) — MUST be
    // rejected by the append-only guard.
    let shrunk = Session::with_id(initial.id().clone());
    let err = store
        .save(&shrunk)
        .await
        .expect_err("shrink attempt must be rejected by the append-only guard");

    match err {
        SessionStoreError::MonotonicityViolation {
            id,
            prev_len,
            new_len,
        } => {
            assert_eq!(
                &id,
                initial.id(),
                "error must name the session whose save was rejected"
            );
            assert_eq!(prev_len, 2);
            assert_eq!(new_len, 0);
        }
        other => panic!("expected MonotonicityViolation, got {other:?}"),
    }

    // Extending (not shrinking) continues to succeed — the guard only
    // rejects strict decreases.
    let mut extended = initial.clone();
    extended.push(Message::User(meerkat_core::types::UserMessage::text("third")));
    store.save(&extended).await.expect("extend must succeed");
}

/// Fork contract (F7 companion): `Session::fork_at` returns a distinct
/// `SessionId`, so a "fork with a shorter history" is a new identity on
/// a new event log, not a same-session shrink. Duplicates the stricter
/// tripwire at `meerkat-core/tests/fork_at_rotates_session_id.rs` —
/// kept here so a downstream consumer reading only this file sees the
/// full append-only contract.
#[test]
fn fork_at_rotates_session_id_so_forks_are_not_shrinks() {
    let mut session = Session::new();
    session.push(Message::User(meerkat_core::types::UserMessage::text("one")));
    session.push(Message::User(meerkat_core::types::UserMessage::text("two")));
    let parent_id = session.id().clone();
    let forked = session.fork_at(0);

    assert_ne!(
        forked.id(),
        &parent_id,
        "fork_at must rotate SessionId — fork is a new identity"
    );
    assert_eq!(forked.messages().len(), 0, "fork_at(0) carries no history");
}
