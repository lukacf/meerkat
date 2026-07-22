//! Capability discovery: the `as_incremental` swallow test.
//!
//! `SessionStore::as_incremental` has a default returning `None`, and
//! `PersistentSessionService` probes it exactly **once at construction**.
//! A `SessionStore` → `SessionStore` delegating wrapper that forwards every
//! async method but not `as_incremental` therefore silently downgrades the
//! whole service to whole-blob persistence. This chapter makes that silent
//! downgrade loud.
//!
//! NOTE the subject is `SessionStore` → `SessionStore` wrappers.
//! `meerkat_store::StoreAdapter` is NOT a subject: it adapts to the
//! two-method `AgentSessionStore` (agent-loop checkpointing), which has no
//! capability accessor to swallow.

use std::sync::Arc;

use meerkat_core::SessionStore;

use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "capability_discovery";

/// Assert that a delegating wrapper forwards the `as_incremental` capability
/// of its inner store.
///
/// `inner` must expose the incremental capability (`as_incremental()` is
/// `Some`) — otherwise the forwarding assertion would be vacuous, and the
/// chapter fails loudly instead of passing silently. `wrap` builds the
/// wrapper under test around a handle to the inner store.
pub async fn assert_forwards_incremental<W>(
    inner: Arc<dyn SessionStore>,
    wrap: W,
) -> Result<(), ConformanceFailure>
where
    W: Fn(Arc<dyn SessionStore>) -> Arc<dyn SessionStore>,
{
    let steps = Steps::chapter(CHAPTER);

    // Vacuity guard: the assertion is only meaningful over an incremental
    // inner store.
    let step = "inner_capability_present";
    steps.ensure(
        step,
        Arc::clone(&inner).as_incremental().is_some(),
        "inner store does not expose the incremental capability; the forwarding assertion \
         would be vacuous — run this chapter with an incremental inner store",
    )?;

    let wrapped = wrap(Arc::clone(&inner));

    // Delegation smoke test: the wrapper must actually delegate to `inner`
    // (a wrapper that writes elsewhere would pass the capability check while
    // failing the storage contract).
    let step = "wrapper_delegates_storage";
    let session = fixtures::session_with_texts(&["capability probe"]);
    steps.wrap(step, wrapped.save(&session).await)?;
    steps.ensure(
        step,
        steps.wrap(step, inner.load(session.id()).await)?.is_some(),
        "a session saved through the wrapper must be visible through the inner store",
    )?;

    // The swallow test proper.
    let step = "as_incremental_forwarded";
    steps.ensure(
        step,
        wrapped.clone().as_incremental().is_some(),
        "delegating wrapper swallows as_incremental: the inner store exposes the incremental \
         capability but the wrapper returns None. PersistentSessionService probes this \
         capability once at construction and silently degrades to whole-blob persistence — \
         forward `as_incremental` in every SessionStore-to-SessionStore delegating wrapper",
    )?;
    Ok(())
}
