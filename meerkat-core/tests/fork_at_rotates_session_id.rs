//! Tripwire for wave-c (Section 1.5 #1). Flipped green by **C-H1** (F7
//! closure from the state-scope-audit — `SessionStore` append-only +
//! `fork_at` becomes an explicit fork, not a same-session truncation).
//!
//! Invariant: `Session::fork_at(...)` must return a session with a
//! **distinct** `SessionId` from its parent. The whole point of the F7
//! typed-witness plan is to make `fork_at` a first-class "fork", i.e. a
//! new identity on a new event log, rather than a same-session
//! truncation.
//!
//! Predicted failure mode today: per the plan, `fork_at` at
//! `meerkat-core/src/session.rs:798` was described as preserving the
//! parent's `SessionId`. If the current implementation already rotates
//! via `SessionId::new()`, this test will pass today — which is itself
//! a signal that the plan's baseline assumption about F7 has shifted.
//! In that case the test serves as a regression guard rather than a
//! failing tripwire, and the c.0 report captures the observation.
//!
//! Catching assertion: parent and fork must not share `id`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_core::Session;

#[test]
fn fork_at_returns_distinct_session_id() {
    let session = Session::new();
    let parent_id = session.id().clone();
    let forked = session.fork_at(0);

    assert_ne!(
        forked.id(),
        &parent_id,
        "Session::fork_at must rotate SessionId (F7 closure, C-H1). \
         A fork of a session is a new identity, not a same-session truncation."
    );
}
