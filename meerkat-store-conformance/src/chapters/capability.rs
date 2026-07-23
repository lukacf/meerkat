//! Capability discovery: the `as_incremental` swallow test.
//!
//! `SessionStore::as_incremental` has a default returning `None`, and
//! `PersistentSessionService` probes it exactly **once at construction**.
//! A `SessionStore` → `SessionStore` delegating wrapper that forwards every
//! async method but not `as_incremental` therefore silently downgrades the
//! whole service to whole-blob persistence. This chapter makes that silent
//! downgrade loud — and proves the forwarded capability is a view over the
//! SAME storage, not a parallel store that merely answers `Some`.
//!
//! NOTE the subject is `SessionStore` → `SessionStore` wrappers.
//! `meerkat_store::StoreAdapter` is NOT a subject: it adapts to the
//! two-method `AgentSessionStore` (agent-loop checkpointing), which has no
//! capability accessor to swallow.

use std::sync::Arc;

use meerkat_core::{
    SessionHead, SessionHeadCas, SessionStore, TranscriptStrandId, transcript_messages_digest,
};

use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "capability_discovery";

/// Assert that a delegating wrapper forwards the `as_incremental` capability
/// of its inner store, and that the forwarded capability shares storage
/// identity with the inner store (writes through one side are served by the
/// other).
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
    let inner_inc = Arc::clone(&inner).as_incremental().ok_or_else(|| {
        steps.fail(
            step,
            "inner store does not expose the incremental capability; the forwarding assertion \
             would be vacuous — run this chapter with an incremental inner store",
        )
    })?;

    let wrapped = wrap(Arc::clone(&inner));

    // Delegation smoke test: the wrapper must actually delegate to `inner`
    // (a wrapper that writes elsewhere would pass the capability check while
    // failing the storage contract). Content equality, not mere presence.
    let step = "wrapper_delegates_storage";
    let session = fixtures::session_with_texts(&["capability probe one", "capability probe two"]);
    steps.wrap(step, wrapped.save(&session).await)?;
    let through_inner = steps
        .wrap(step, inner.load(session.id()).await)?
        .ok_or_else(|| {
            steps.fail(
                step,
                "a session saved through the wrapper must be visible through the inner store",
            )
        })?;
    let saved_revision = steps.wrap(step, transcript_messages_digest(session.messages()))?;
    steps.ensure(
        step,
        steps.wrap(step, transcript_messages_digest(through_inner.messages()))? == saved_revision,
        "the inner store must serve exactly the transcript saved through the wrapper",
    )?;

    // The swallow test proper.
    let step = "as_incremental_forwarded";
    let wrapped_inc = Arc::clone(&wrapped).as_incremental().ok_or_else(|| {
        steps.fail(
            step,
            "delegating wrapper swallows as_incremental: the inner store exposes the incremental \
             capability but the wrapper returns None. PersistentSessionService probes this \
             capability once at construction and silently degrades to whole-blob persistence — \
             forward `as_incremental` in every SessionStore-to-SessionStore delegating wrapper",
        )
    })?;

    // Storage identity: `Some` is not enough — the forwarded capability must
    // be backed by the SAME storage as the inner store. Write through the
    // wrapper's capability, read through the inner side (and vice versa),
    // asserting identical data.
    let step = "capability_shares_storage_identity";
    let root = TranscriptStrandId::root();

    // Direction 1: wrapper capability writes → inner store reads.
    let forward = fixtures::session_with_texts(&["identity forward one", "identity forward two"]);
    steps.wrap(
        step,
        wrapped_inc
            .append_messages(forward.id(), &root, 0, forward.messages())
            .await,
    )?;
    let forward_head = steps.wrap(step, SessionHead::from_session(&forward, root.clone(), 0))?;
    steps.wrap(
        step,
        wrapped_inc
            .save_head(&forward_head, SessionHeadCas::Create)
            .await,
    )?;
    let via_inner = steps
        .wrap(step, inner.load(forward.id()).await)?
        .ok_or_else(|| {
            steps.fail(
                step,
                "a session written through the wrapper's incremental capability must be served \
                 by the inner store — the forwarded capability is backed by a different store",
            )
        })?;
    steps.ensure(
        step,
        via_inner.messages() == forward.messages(),
        "the inner store must serve exactly the messages written through the wrapper's \
         incremental capability",
    )?;
    let via_inner_head = steps
        .wrap(step, inner_inc.load_head(forward.id()).await)?
        .ok_or_else(|| {
            steps.fail(
                step,
                "the head written through the wrapper's capability must load through the inner \
                 store's capability",
            )
        })?;
    steps.ensure(
        step,
        via_inner_head.head_revision == forward_head.head_revision
            && via_inner_head.message_count == forward_head.message_count,
        "the head row served by the inner capability must be the head written through the \
         wrapper's capability",
    )?;

    // Direction 2: inner capability writes → wrapper capability/base reads.
    let reverse = fixtures::session_with_texts(&["identity reverse one"]);
    steps.wrap(
        step,
        inner_inc
            .append_messages(reverse.id(), &root, 0, reverse.messages())
            .await,
    )?;
    let reverse_head = steps.wrap(step, SessionHead::from_session(&reverse, root, 0))?;
    steps.wrap(
        step,
        inner_inc
            .save_head(&reverse_head, SessionHeadCas::Create)
            .await,
    )?;
    let via_wrapper_head = steps
        .wrap(step, wrapped_inc.load_head(reverse.id()).await)?
        .ok_or_else(|| {
            steps.fail(
                step,
                "a head written through the inner capability must load through the wrapper's \
                 capability — the forwarded capability is backed by a different store",
            )
        })?;
    steps.ensure(
        step,
        via_wrapper_head.head_revision == reverse_head.head_revision
            && via_wrapper_head.message_count == reverse_head.message_count,
        "the head row served by the wrapper's capability must be the head written through the \
         inner capability",
    )?;
    let via_wrapper = steps
        .wrap(step, wrapped.load(reverse.id()).await)?
        .ok_or_else(|| {
            steps.fail(
                step,
                "a session written through the inner capability must be served by the wrapper",
            )
        })?;
    steps.ensure(
        step,
        via_wrapper.messages() == reverse.messages(),
        "the wrapper must serve exactly the messages written through the inner capability",
    )?;
    Ok(())
}
