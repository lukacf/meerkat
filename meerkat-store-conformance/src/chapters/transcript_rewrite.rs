//! Transcript-rewrite chapter: the whole-blob `save_transcript_rewrite`
//! contract.
//!
//! `save_transcript_rewrite` is the ONLY `SessionStore` path allowed to
//! replace or shrink the current message projection — it is the primary
//! compaction path for whole-blob backends (JSONL). This chapter pins:
//!
//! - a compaction-shaped rewrite WITHOUT proof (a plain `save` of the
//!   rewritten document) is refused with a typed error and leaves the
//!   committed row untouched;
//! - the same rewrite WITH its [`TranscriptRewriteCommit`] proof is
//!   accepted, post-rewrite reads serve exactly the rewritten transcript,
//!   the rewritten head takes ordinary appends, and everything survives
//!   reopen;
//! - a STALE proof (the persisted head advanced past the commit's parent)
//!   is refused with `TranscriptRevisionConflict` and leaves the row
//!   untouched.
//!
//! Invoke this chapter only for stores that implement
//! `save_transcript_rewrite` (the trait default refuses with a typed
//! internal error, which fails the chapter loudly rather than vacuously).

use meerkat_core::{
    Message, Session, SessionStore, SessionStoreError, TranscriptRewriteCommit,
    TranscriptRewriteReason, TranscriptRewriteSelection, UserMessage, transcript_messages_digest,
};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "transcript_rewrite";

/// Transcript-rewrite chapter (see module docs).
pub async fn transcript_rewrite(
    factory: &dyn SessionStoreFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    rewrite_without_proof_refused(&steps, store.as_ref()).await?;
    rewrite_with_proof_round_trip(&steps, factory, store.as_ref()).await?;
    stale_proof_refused(&steps, store.as_ref()).await?;
    Ok(())
}

/// Produce the compaction shape: replace the first two messages of `session`
/// with one summary message, returning the rewritten session and its typed
/// commit proof.
fn compacted(
    steps: &Steps,
    step: &'static str,
    session: &Session,
) -> Result<(Session, TranscriptRewriteCommit), ConformanceFailure> {
    let mut rewritten = session.clone();
    let commit = steps.wrap(
        step,
        rewritten.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::text(
                "[conformance] compacted summary".to_string(),
            ))],
            TranscriptRewriteReason::new("conformance"),
            Some("meerkat-store-conformance".to_string()),
            None,
        ),
    )?;
    Ok((rewritten, commit))
}

async fn served_digest(
    steps: &Steps,
    step: &'static str,
    store: &dyn SessionStore,
    session: &Session,
) -> Result<String, ConformanceFailure> {
    let served = steps
        .wrap(step, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(step, "session row must load"))?;
    steps.wrap(step, transcript_messages_digest(served.messages()))
}

async fn rewrite_without_proof_refused(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "rewrite_without_proof_refused";
    let session = fixtures::session_with_texts(&["one", "two", "three"]);
    steps.wrap(STEP, store.save(&session).await)?;
    let committed_digest = served_digest(steps, STEP, store, &session).await?;

    // The exact document the proven path accepts below must be refused on
    // the plain save path: shrink/replace without a commit proof.
    let (rewritten, _commit) = compacted(steps, STEP, &session)?;
    match store.save(&rewritten).await {
        Err(
            SessionStoreError::MonotonicityViolation { .. }
            | SessionStoreError::InvalidTranscriptRewrite { .. }
            | SessionStoreError::TranscriptContinuityViolation { .. },
        ) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "unproven rewrite save must fail with a typed monotonicity/rewrite error, \
                     got: {other}"
                ),
            ));
        }
        Ok(()) => {
            return Err(steps.fail(
                STEP,
                "a transcript rewrite without its commit proof must be rejected by plain save",
            ));
        }
    }

    // The refusal must leave the committed row exactly as it was.
    let after = served_digest(steps, STEP, store, &session).await?;
    steps.ensure(
        STEP,
        after == committed_digest,
        "a refused unproven rewrite must leave the persisted row untouched",
    )?;
    Ok(())
}

async fn rewrite_with_proof_round_trip(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "rewrite_with_proof_round_trip";
    let session = fixtures::session_with_texts(&["one", "two", "three"]);
    steps.wrap(STEP, store.save(&session).await)?;

    let (rewritten, commit) = compacted(steps, STEP, &session)?;
    steps.wrap(
        STEP,
        store.save_transcript_rewrite(&rewritten, &commit).await,
    )?;

    // Post-rewrite reads serve exactly the rewritten transcript (the commit
    // revision digest is content identity, not just a length).
    let served = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "rewritten session must load"))?;
    steps.ensure(
        STEP,
        served.messages().len() == commit.messages_after,
        "post-rewrite load must serve exactly messages_after messages",
    )?;
    let served_revision = steps.wrap(STEP, transcript_messages_digest(served.messages()))?;
    steps.ensure(
        STEP,
        served_revision == commit.revision,
        "post-rewrite load must serve exactly the commit's revision transcript",
    )?;

    // The rewritten head takes ordinary appends.
    let mut continued = served;
    fixtures::push_text(&mut continued, "post-rewrite continuation");
    steps.wrap(STEP, store.save(&continued).await)?;
    let continued_digest = steps.wrap(STEP, transcript_messages_digest(continued.messages()))?;
    steps.ensure(
        STEP,
        served_digest(steps, STEP, store, &continued).await? == continued_digest,
        "the post-rewrite continuation must be served after a plain save",
    )?;

    // Restart survival: the rewrite outcome must survive reopen.
    let reopened = factory.open().await?;
    let survived = steps
        .wrap(STEP, reopened.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "rewritten session must survive reopen"))?;
    let survived_digest = steps.wrap(STEP, transcript_messages_digest(survived.messages()))?;
    steps.ensure(
        STEP,
        survived_digest == continued_digest,
        "the rewritten + continued transcript must survive reopen",
    )?;
    Ok(())
}

async fn stale_proof_refused(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "stale_proof_refused";
    let mut session = fixtures::session_with_texts(&["one", "two", "three"]);
    steps.wrap(STEP, store.save(&session).await)?;
    let (rewritten, commit) = compacted(steps, STEP, &session)?;

    // An intervening append advances the persisted head past the commit's
    // parent revision.
    fixtures::push_text(&mut session, "four");
    steps.wrap(STEP, store.save(&session).await)?;
    let committed_digest = served_digest(steps, STEP, store, &session).await?;

    match store.save_transcript_rewrite(&rewritten, &commit).await {
        Err(SessionStoreError::TranscriptRevisionConflict { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "a stale rewrite proof must fail with TranscriptRevisionConflict, got: {other}"
                ),
            ));
        }
        Ok(()) => {
            return Err(steps.fail(
                STEP,
                "a rewrite whose proof parents a superseded head must be rejected",
            ));
        }
    }
    let after = served_digest(steps, STEP, store, &session).await?;
    steps.ensure(
        STEP,
        after == committed_digest,
        "a refused stale rewrite must leave the persisted row untouched",
    )?;
    Ok(())
}
