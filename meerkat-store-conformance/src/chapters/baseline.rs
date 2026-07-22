//! Baseline `SessionStore` chapter: the contract every backend must satisfy.

use std::sync::Arc;

use meerkat_core::session_store::session_projection_cas_token;
use meerkat_core::{
    Message, SessionCheckpointRevision, SessionCheckpointState, SessionFilter, SessionGeneration,
    SessionId, SessionStore, SessionStoreError, UserMessage, adopt_legacy_session,
};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "baseline";

/// Number of concurrent writers in the contention step.
const CONCURRENT_WRITERS: usize = 8;
/// Bounded retry budget per writer (no timing dependence: writers retry on
/// typed continuity conflicts only).
const CONCURRENT_WRITE_ATTEMPTS: usize = 200;

/// Baseline chapter: save/load round-trips, list/delete,
/// `delete_if_current_revision` guard semantics, append-only save-guard
/// enforcement, checkpoint-stamp preservation across save/load,
/// concurrent-writer contention, and large payloads.
pub async fn baseline(factory: &dyn SessionStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    save_load_round_trip(&steps, factory, store.as_ref()).await?;
    list_and_delete(&steps, store.as_ref()).await?;
    append_only_guard(&steps, store.as_ref()).await?;
    delete_if_current_revision_guard(&steps, store.as_ref()).await?;
    checkpoint_stamp_round_trip(&steps, factory, store.as_ref()).await?;
    concurrent_writer_contention(&steps, Arc::clone(&store)).await?;
    large_payload(&steps, store.as_ref()).await?;
    Ok(())
}

async fn save_load_round_trip(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "save_load_round_trip";
    let mut session = fixtures::session_with_texts(&["turn one", "turn two"]);
    session.set_metadata(
        "conformance_marker",
        serde_json::json!({"chapter": CHAPTER}),
    );
    steps.wrap(STEP, store.save(&session).await)?;

    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "saved session must load back"))?;
    steps.ensure(STEP, loaded.id() == session.id(), "loaded id must match")?;
    steps.ensure(
        STEP,
        loaded.messages().len() == 2,
        format!(
            "loaded message count {} != saved 2",
            loaded.messages().len()
        ),
    )?;
    let saved_revision = steps.wrap(STEP, session.transcript_revision())?;
    let loaded_revision = steps.wrap(STEP, loaded.transcript_revision())?;
    steps.ensure(
        STEP,
        saved_revision == loaded_revision,
        "loaded transcript revision must match the saved transcript",
    )?;
    steps.ensure(
        STEP,
        loaded.metadata().get("conformance_marker")
            == Some(&serde_json::json!({"chapter": CHAPTER})),
        "session metadata must round-trip",
    )?;

    // exists / load_meta agree with load.
    steps.ensure(
        STEP,
        steps.wrap(STEP, store.exists(session.id()).await)?,
        "exists must report true for a saved session",
    )?;
    let absent = SessionId::new();
    steps.ensure(
        STEP,
        !steps.wrap(STEP, store.exists(&absent).await)?,
        "exists must report false for an absent session",
    )?;
    let meta = steps
        .wrap(STEP, store.load_meta(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "load_meta must return the saved session's metadata"))?;
    steps.ensure(
        STEP,
        meta.message_count == 2,
        "load_meta message_count must match the persisted transcript",
    )?;

    // Restart survival: a reopened handle over the same storage serves the
    // same row (a shared-state handle for non-persistent backends).
    let reopened = factory.open().await?;
    let survived = steps
        .wrap(STEP, reopened.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "session must survive a reopen of the same storage"))?;
    let survived_revision = steps.wrap(STEP, survived.transcript_revision())?;
    steps.ensure(
        STEP,
        survived_revision == saved_revision,
        "reopened handle must serve the same transcript revision",
    )?;
    Ok(())
}

async fn list_and_delete(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "list_and_delete";
    let session = fixtures::session_with_texts(&["listable"]);
    steps.wrap(STEP, store.save(&session).await)?;

    let listed = steps.wrap(STEP, store.list(SessionFilter::default()).await)?;
    let meta = listed
        .iter()
        .find(|meta| &meta.id == session.id())
        .ok_or_else(|| steps.fail(STEP, "list must include the saved session"))?;
    steps.ensure(
        STEP,
        meta.message_count == 1,
        "listed message_count must match the persisted transcript",
    )?;

    steps.wrap(STEP, store.delete(session.id()).await)?;
    steps.ensure(
        STEP,
        steps.wrap(STEP, store.load(session.id()).await)?.is_none(),
        "deleted session must not load",
    )?;
    steps.ensure(
        STEP,
        !steps.wrap(STEP, store.exists(session.id()).await)?,
        "deleted session must not exist",
    )?;
    let relisted = steps.wrap(STEP, store.list(SessionFilter::default()).await)?;
    steps.ensure(
        STEP,
        !relisted.iter().any(|meta| &meta.id == session.id()),
        "deleted session must not be listed",
    )?;
    Ok(())
}

async fn append_only_guard(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "append_only_guard";
    let mut session = fixtures::session_with_texts(&["one", "two", "three"]);
    steps.wrap(STEP, store.save(&session).await)?;

    // Prefix-preserving append is admitted.
    fixtures::push_text(&mut session, "four");
    steps.wrap(STEP, store.save(&session).await)?;

    // Shrink without a transcript-continuity proof must be rejected with the
    // typed MonotonicityViolation, and the persisted row must be untouched.
    let truncated = fixtures::with_transcript_truncated(&session, 2)?;
    match store.save(&truncated).await {
        Err(SessionStoreError::MonotonicityViolation { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!("shrink must fail with MonotonicityViolation, got: {other}"),
            ));
        }
        Ok(()) => {
            return Err(steps.fail(STEP, "shrink save must be rejected"));
        }
    }

    // Same-length divergence (not a continuation) must be rejected with the
    // typed TranscriptContinuityViolation.
    let divergent = fixtures::with_divergent_tail(&session, "divergent four")?;
    match store.save(&divergent).await {
        Err(SessionStoreError::TranscriptContinuityViolation { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "divergent save must fail with TranscriptContinuityViolation, got: {other}"
                ),
            ));
        }
        Ok(()) => {
            return Err(steps.fail(STEP, "divergent save must be rejected"));
        }
    }

    // Rejected saves must leave the persisted row exactly as committed.
    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "session must still load after rejected saves"))?;
    steps.ensure(
        STEP,
        loaded.messages().len() == 4,
        "rejected saves must not mutate the persisted row",
    )?;
    Ok(())
}

async fn delete_if_current_revision_guard(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "delete_if_current_revision_guard";
    let mut session = fixtures::session_with_texts(&["base"]);
    steps.wrap(STEP, store.save(&session).await)?;
    let stale_row = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "saved session must load"))?;
    let stale_token = steps.wrap(STEP, session_projection_cas_token(&stale_row))?;

    fixtures::push_text(&mut session, "newer");
    steps.wrap(STEP, store.save(&session).await)?;

    // Stale token: no delete, row intact.
    steps.ensure(
        STEP,
        !steps.wrap(
            STEP,
            store
                .delete_if_current_revision(session.id(), &stale_token)
                .await,
        )?,
        "delete_if_current_revision with a stale token must return false",
    )?;
    steps.ensure(
        STEP,
        steps.wrap(STEP, store.load(session.id()).await)?.is_some(),
        "a stale-token delete must leave the row in place",
    )?;

    // Current token (computed over the row the store actually serves).
    let current_row = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "current row must load"))?;
    let current_token = steps.wrap(STEP, session_projection_cas_token(&current_row))?;
    steps.ensure(
        STEP,
        steps.wrap(
            STEP,
            store
                .delete_if_current_revision(session.id(), &current_token)
                .await,
        )?,
        "delete_if_current_revision with the current token must return true",
    )?;
    steps.ensure(
        STEP,
        steps.wrap(STEP, store.load(session.id()).await)?.is_none(),
        "a current-token delete must remove the row",
    )?;

    // Absent session: false, not an error.
    steps.ensure(
        STEP,
        !steps.wrap(
            STEP,
            store
                .delete_if_current_revision(&SessionId::new(), &current_token)
                .await,
        )?,
        "delete_if_current_revision on an absent session must return false",
    )?;
    Ok(())
}

async fn checkpoint_stamp_round_trip(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "checkpoint_stamp_round_trip";
    let session = fixtures::session_with_texts(&["stamped turn"]);
    let blob = fixtures::legacy_session_blob(&session)?;
    let adopted = steps.wrap(
        STEP,
        adopt_legacy_session(
            &blob,
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
        ),
    )?;

    steps.wrap(STEP, store.save(&adopted.session).await)?;
    let reopened = factory.open().await?;
    let loaded = steps
        .wrap(STEP, reopened.load(adopted.session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "stamped session must load after reopen"))?;
    match steps.wrap(STEP, loaded.try_checkpoint_state())? {
        SessionCheckpointState::Verified(stamp) if stamp == adopted.stamp => Ok(()),
        SessionCheckpointState::Verified(_) => Err(steps.fail(
            STEP,
            "loaded checkpoint stamp must equal the adopted stamp byte-for-byte",
        )),
        SessionCheckpointState::LegacyUnverified { .. } => Err(steps.fail(
            STEP,
            "a stamped (adopted) session must stay Verified across save/load — the store dropped \
             or mangled the reserved checkpoint-stamp metadata",
        )),
    }
}

async fn concurrent_writer_contention(
    steps: &Steps,
    store: Arc<dyn SessionStore>,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "concurrent_writer_contention";
    let base = fixtures::session_with_texts(&["contended base"]);
    steps.wrap(STEP, store.save(&base).await)?;
    let id = base.id().clone();

    let mut joins = Vec::with_capacity(CONCURRENT_WRITERS);
    for writer in 0..CONCURRENT_WRITERS {
        let store = Arc::clone(&store);
        let id = id.clone();
        joins.push(tokio::spawn(async move {
            let marker = format!("writer-{writer}");
            for _attempt in 0..CONCURRENT_WRITE_ATTEMPTS {
                let current = match store.load(&id).await {
                    Ok(Some(current)) => current,
                    Ok(None) => return Err("session vanished under contention".to_string()),
                    Err(error) => return Err(format!("load failed under contention: {error}")),
                };
                let mut next = current;
                next.push(Message::User(UserMessage::text(marker.clone())));
                match store.save(&next).await {
                    Ok(()) => return Ok(()),
                    Err(
                        SessionStoreError::TranscriptContinuityViolation { .. }
                        | SessionStoreError::MonotonicityViolation { .. },
                    ) => {
                        // Lost the race: the guard held. Reload and retry.
                    }
                    Err(other) => {
                        return Err(format!(
                            "contended save failed with a non-continuity error: {other}"
                        ));
                    }
                }
            }
            Err("writer exhausted its retry budget".to_string())
        }));
    }
    for join in joins {
        match join.await {
            Ok(Ok(())) => {}
            Ok(Err(detail)) => return Err(steps.fail(STEP, detail)),
            Err(join_error) => {
                return Err(steps.fail(STEP, format!("writer task panicked: {join_error}")));
            }
        }
    }

    // Every writer that reported success must be visible exactly once: the
    // guard must linearize appends, never silently drop a committed write.
    let final_row = steps
        .wrap(STEP, store.load(&id).await)?
        .ok_or_else(|| steps.fail(STEP, "contended session must load"))?;
    steps.ensure(
        STEP,
        final_row.messages().len() == 1 + CONCURRENT_WRITERS,
        format!(
            "expected {} messages after {} successful appends, found {}",
            1 + CONCURRENT_WRITERS,
            CONCURRENT_WRITERS,
            final_row.messages().len()
        ),
    )?;
    for writer in 0..CONCURRENT_WRITERS {
        let marker = format!("writer-{writer}");
        let occurrences = final_row
            .messages()
            .iter()
            .filter(|message| match message {
                Message::User(user) => user
                    .content
                    .iter()
                    .any(|block| block.text_projection() == marker),
                _ => false,
            })
            .count();
        steps.ensure(
            STEP,
            occurrences == 1,
            format!("marker {marker} must appear exactly once, found {occurrences}"),
        )?;
    }
    Ok(())
}

async fn large_payload(steps: &Steps, store: &dyn SessionStore) -> Result<(), ConformanceFailure> {
    const STEP: &str = "large_payload";
    // ~4 MiB across several messages plus one ~2 MiB single message.
    let chunk = fixtures::large_text(512 * 1024);
    let mut session = fixtures::session_with_texts(&[]);
    for _ in 0..4 {
        fixtures::push_text(&mut session, &chunk);
    }
    let big = fixtures::large_text(2 * 1024 * 1024);
    fixtures::push_text(&mut session, &big);
    let saved_revision = steps.wrap(STEP, session.transcript_revision())?;

    steps.wrap(STEP, store.save(&session).await)?;
    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "multi-MB session must load back"))?;
    let loaded_revision = steps.wrap(STEP, loaded.transcript_revision())?;
    steps.ensure(
        STEP,
        loaded_revision == saved_revision,
        "multi-MB transcript must round-trip byte-exact (revision mismatch)",
    )?;

    // Large sessions still take guarded appends.
    let mut extended = loaded;
    fixtures::push_text(&mut extended, "post-large append");
    steps.wrap(STEP, store.save(&extended).await)?;
    Ok(())
}
