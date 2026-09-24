//! Lock-in tests for the 0.6 clean cut of `state: Arc<AtomicU8>` shadow from
//! `MobHandle` and `MobActor`.
//!
//! Dogma #1 (one owner), #13 (projection rebuild trigger must be explicit —
//! always-on AtomicU8 projection is not), #17 (MobHandle surface must not
//! own semantic truth).
//!
//! Before the 0.6 cut, `MobHandle::status()` was a sync `AtomicU8::load`.
//! After the cut it routes through the actor command channel and reads the
//! DSL authority on demand, so it must be `async` and fallible.
//!
//! If either the `state: Arc<AtomicU8>` shadow on `MobHandle` or the
//! `lifecycle_phase_projection: Arc<AtomicU8>` shadow on `MobActor` is ever
//! re-introduced, `status()` would revert to a sync infallible signature and
//! the compile-time assertions below would fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use meerkat_mob::runtime::MobHandle;
use meerkat_mob::{MobError, MobState};

/// Compile-time proof that `MobHandle::status()` is async and fallible — it
/// must return a `Future<Output = Result<MobState, MobError>>`. If the
/// `AtomicU8` shadow re-lands, `status()` would collapse back to
/// `fn status(&self) -> MobState`, breaking this coercion at compile time.
#[allow(dead_code)]
fn _prove_status_is_async(
    h: &MobHandle,
) -> impl std::future::Future<Output = Result<MobState, MobError>> + '_ {
    h.status()
}

/// Sanity test: the file wires up and the compile-time proof compiles.
/// The actual behavioural assertion is statically typed above.
#[tokio::test]
async fn status_method_signature_is_async_fallible() {
    // Typecheck-only: if `MobHandle::status` ever reverts to sync-infallible
    // `-> MobState`, `_prove_status_is_async` would fail to compile and this
    // whole test binary would break.
    let _ = _prove_status_is_async;
}
