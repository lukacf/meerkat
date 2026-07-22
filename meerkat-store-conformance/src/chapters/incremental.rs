//! Incremental profile: the `IncrementalSessionStore` capability contract.

use std::sync::Arc;

use meerkat_core::session_store::{IncrementalSessionStore, session_head_cas_token};
use meerkat_core::{
    Message, Session, SessionHead, SessionHeadCas, SessionStoreError, TranscriptRewriteReason,
    TranscriptRewriteRecord, TranscriptRewriteSelection, TranscriptStrandId, UserMessage,
};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "incremental";

/// Incremental profile: O(delta) `append_messages` semantics, CAS-guarded
/// `save_head` (`Create` / `IfToken`), `commit_rewrite` CAS against the head
/// token, `TranscriptRevisionConflict` on token/parent mismatch, and
/// `load_messages` / `load_rewrites` round-trips.
///
/// Invoke this chapter only for stores whose `as_incremental` returns
/// `Some`; invoking it for a store without the capability fails loudly.
pub async fn incremental(factory: &dyn SessionStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;
    let Some(inc) = Arc::clone(&store).as_incremental() else {
        return Err(steps.fail(
            "capability_probe",
            "incremental profile invoked for a store whose as_incremental() returned None",
        ));
    };

    append_and_head_create(&steps, &store, inc.as_ref()).await?;
    append_contract(&steps, inc.as_ref()).await?;
    save_head_cas(&steps, inc.as_ref()).await?;
    rewrite_commit_and_adoption(&steps, factory, &store, inc.as_ref()).await?;
    Ok(())
}

/// Seed one session through the incremental write path and return
/// `(session, head, head_token)` as stored.
async fn seed(
    steps: &Steps,
    step: &'static str,
    inc: &dyn IncrementalSessionStore,
    texts: &[&str],
) -> Result<(Session, SessionHead, String), ConformanceFailure> {
    let session = fixtures::session_with_texts(texts);
    let root = TranscriptStrandId::root();
    steps.wrap(
        step,
        inc.append_messages(session.id(), &root, 0, session.messages())
            .await,
    )?;
    let head = steps.wrap(step, SessionHead::from_session(&session, root, 0))?;
    steps.wrap(step, inc.save_head(&head, SessionHeadCas::Create).await)?;
    let token = steps.wrap(step, session_head_cas_token(&head))?;
    Ok((session, head, token))
}

async fn append_and_head_create(
    steps: &Steps,
    store: &Arc<dyn meerkat_core::SessionStore>,
    inc: &dyn IncrementalSessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "append_and_head_create";
    let (session, head, _token) = seed(steps, STEP, inc, &["one", "two"]).await?;

    let loaded_head = steps
        .wrap(STEP, inc.load_head(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "saved head must load back"))?;
    steps.ensure(
        STEP,
        loaded_head.head_revision == head.head_revision && loaded_head.message_count == 2,
        "loaded head must match the saved head row",
    )?;

    // The compat read path serves the slim materialization of the head.
    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "incremental session must load via SessionStore::load"))?;
    steps.ensure(
        STEP,
        loaded.messages().len() == 2,
        "slim load must serve exactly the head-covered messages",
    )?;

    // load_messages round-trips: full range and sub-range.
    let root = TranscriptStrandId::root();
    let all = steps.wrap(STEP, inc.load_messages(session.id(), &root, 0..2).await)?;
    steps.ensure(
        STEP,
        all.len() == 2,
        "full-range load_messages must serve both rows",
    )?;
    let tail = steps.wrap(STEP, inc.load_messages(session.id(), &root, 1..2).await)?;
    steps.ensure(
        STEP,
        tail.len() == 1,
        "sub-range load_messages must serve one row",
    )?;
    steps.ensure(
        STEP,
        inc.load_messages(session.id(), &root, 0..9).await.is_err(),
        "out-of-range load_messages must fail closed",
    )?;
    Ok(())
}

async fn append_contract(
    steps: &Steps,
    inc: &dyn IncrementalSessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "append_contract";
    let (mut session, head, token) = seed(steps, STEP, inc, &["one", "two"]).await?;
    let root = TranscriptStrandId::root();

    // O(delta) append: only the new row travels.
    fixtures::push_text(&mut session, "three");
    let delta = &session.messages()[2..];
    steps.wrap(
        STEP,
        inc.append_messages(session.id(), &root, 2, delta).await,
    )?;
    let new_head = steps.wrap(
        STEP,
        SessionHead::from_session(&session, root.clone(), head.rewrite_count),
    )?;
    steps.wrap(
        STEP,
        inc.save_head(&new_head, SessionHeadCas::IfToken(token))
            .await,
    )?;

    // Idempotency: re-appending identical bytes at the same seq is Ok.
    steps.wrap(
        STEP,
        inc.append_messages(session.id(), &root, 2, delta).await,
    )?;

    // Contiguity: a gap append fails closed.
    match inc
        .append_messages(
            session.id(),
            &root,
            9,
            &[Message::User(UserMessage::text("gap".to_string()))],
        )
        .await
    {
        Err(SessionStoreError::TranscriptContinuityViolation { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!("gap append must fail with TranscriptContinuityViolation, got: {other}"),
            ));
        }
        Ok(()) => return Err(steps.fail(STEP, "gap append must be rejected")),
    }

    // Immutability: overwriting an existing (strand, seq) row with different
    // bytes fails closed.
    match inc
        .append_messages(
            session.id(),
            &root,
            0,
            &[Message::User(UserMessage::text(
                "divergent one".to_string(),
            ))],
        )
        .await
    {
        Err(SessionStoreError::TranscriptContinuityViolation { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "divergent overwrite must fail with TranscriptContinuityViolation, got: {other}"
                ),
            ));
        }
        Ok(()) => return Err(steps.fail(STEP, "divergent row overwrite must be rejected")),
    }
    Ok(())
}

async fn save_head_cas(
    steps: &Steps,
    inc: &dyn IncrementalSessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "save_head_cas";
    let (mut session, head, token) = seed(steps, STEP, inc, &["one", "two"]).await?;
    let root = TranscriptStrandId::root();

    // Create when a head already exists must conflict.
    match inc.save_head(&head, SessionHeadCas::Create).await {
        Err(SessionStoreError::TranscriptRevisionConflict { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "Create over an existing head must fail with TranscriptRevisionConflict, got: {other}"
                ),
            ));
        }
        Ok(()) => return Err(steps.fail(STEP, "Create over an existing head must be rejected")),
    }

    // Advance once so `token` goes stale.
    fixtures::push_text(&mut session, "three");
    steps.wrap(
        STEP,
        inc.append_messages(session.id(), &root, 2, &session.messages()[2..])
            .await,
    )?;
    let advanced = steps.wrap(
        STEP,
        SessionHead::from_session(&session, root.clone(), head.rewrite_count),
    )?;
    steps.wrap(
        STEP,
        inc.save_head(&advanced, SessionHeadCas::IfToken(token.clone()))
            .await,
    )?;

    // A stale token must conflict.
    fixtures::push_text(&mut session, "four");
    steps.wrap(
        STEP,
        inc.append_messages(session.id(), &root, 3, &session.messages()[3..])
            .await,
    )?;
    let next = steps.wrap(
        STEP,
        SessionHead::from_session(&session, root.clone(), head.rewrite_count),
    )?;
    match inc.save_head(&next, SessionHeadCas::IfToken(token)).await {
        Err(SessionStoreError::TranscriptRevisionConflict { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "stale IfToken save_head must fail with TranscriptRevisionConflict, got: {other}"
                ),
            ));
        }
        Ok(()) => return Err(steps.fail(STEP, "stale IfToken save_head must be rejected")),
    }

    // Same-strand shrink must surface MonotonicityViolation even with the
    // correct token.
    let advanced_token = steps.wrap(STEP, session_head_cas_token(&advanced))?;
    let shrunk_source = fixtures::with_transcript_truncated(&session, 1)?;
    let shrunk = steps.wrap(
        STEP,
        SessionHead::from_session(&shrunk_source, root, head.rewrite_count),
    )?;
    match inc
        .save_head(&shrunk, SessionHeadCas::IfToken(advanced_token))
        .await
    {
        Err(SessionStoreError::MonotonicityViolation { .. }) => Ok(()),
        Err(other) => Err(steps.fail(
            STEP,
            format!("same-strand head shrink must fail with MonotonicityViolation, got: {other}"),
        )),
        Ok(()) => Err(steps.fail(STEP, "same-strand head shrink must be rejected")),
    }
}

async fn rewrite_commit_and_adoption(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &Arc<dyn meerkat_core::SessionStore>,
    inc: &dyn IncrementalSessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "rewrite_commit_and_adoption";
    let (session, _head, token) = seed(steps, STEP, inc, &["one", "two"]).await?;

    // Produce a typed rewrite (compaction-shaped range replacement).
    let mut rewritten = session.clone();
    let commit = steps.wrap(
        STEP,
        rewritten.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::text(
                "[conformance] rewritten summary".to_string(),
            ))],
            TranscriptRewriteReason::new("conformance"),
            Some("meerkat-store-conformance".to_string()),
            None,
        ),
    )?;
    let parent_body = steps
        .wrap(
            STEP,
            rewritten.transcript_revision_body(&commit.parent_revision),
        )?
        .ok_or_else(|| steps.fail(STEP, "rewrite must retain the parent revision body"))?;
    let revision_body = steps
        .wrap(STEP, rewritten.transcript_revision_body(&commit.revision))?
        .ok_or_else(|| steps.fail(STEP, "rewrite must retain the new revision body"))?;
    let record = steps.wrap(
        STEP,
        TranscriptRewriteRecord::new(commit.clone(), parent_body, revision_body),
    )?;

    // A stale head token must conflict before anything is written.
    match inc
        .commit_rewrite(
            session.id(),
            &record,
            SessionHeadCas::IfToken("head-sha256:stale".to_string()),
        )
        .await
    {
        Err(SessionStoreError::TranscriptRevisionConflict { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "stale-token commit_rewrite must fail with TranscriptRevisionConflict, got: {other}"
                ),
            ));
        }
        Ok(_) => return Err(steps.fail(STEP, "stale-token commit_rewrite must be rejected")),
    }

    // Correct token: the commit is recorded but NOT adopted until save_head.
    let next = steps.wrap(
        STEP,
        inc.commit_rewrite(
            session.id(),
            &record,
            SessionHeadCas::IfToken(token.clone()),
        )
        .await,
    )?;
    steps.ensure(
        STEP,
        steps
            .wrap(STEP, inc.load_rewrites(session.id()).await)?
            .is_empty(),
        "recorded-but-unadopted rewrites must not be served by load_rewrites",
    )?;

    // Adoption: save_head with the implied next head.
    steps.wrap(
        STEP,
        inc.save_head(&next, SessionHeadCas::IfToken(token)).await,
    )?;
    let rewrites = steps.wrap(STEP, inc.load_rewrites(session.id()).await)?;
    steps.ensure(
        STEP,
        rewrites.len() == 1 && rewrites[0].commit.revision == commit.revision,
        "adopted rewrite must round-trip through load_rewrites",
    )?;

    // The compat read now serves the compacted (slim) transcript.
    let slim = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "rewritten session must load"))?;
    steps.ensure(
        STEP,
        slim.messages().len() == 1,
        "post-adoption load must serve the rewritten transcript",
    )?;

    // A replayed rewrite against the advanced head must conflict.
    match inc
        .commit_rewrite(
            session.id(),
            &record,
            SessionHeadCas::IfToken(steps.wrap(STEP, session_head_cas_token(&next))?),
        )
        .await
    {
        Err(SessionStoreError::TranscriptRevisionConflict { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "replayed rewrite against an advanced head must fail with \
                     TranscriptRevisionConflict, got: {other}"
                ),
            ));
        }
        Ok(_) => {
            return Err(steps.fail(
                STEP,
                "replayed rewrite against an advanced head must be rejected",
            ));
        }
    }

    // Restart survival: head, adopted rewrites, and slim load all reopen.
    let reopened = factory.open().await?;
    let reopened_inc = Arc::clone(&reopened).as_incremental().ok_or_else(|| {
        steps.fail(
            STEP,
            "reopened handle must still expose the incremental capability",
        )
    })?;
    let survived_head = steps
        .wrap(STEP, reopened_inc.load_head(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "head row must survive reopen"))?;
    steps.ensure(
        STEP,
        survived_head.rewrite_count == 1 && survived_head.message_count == 1,
        "reopened head must carry the adopted rewrite and compacted count",
    )?;
    steps.ensure(
        STEP,
        steps
            .wrap(STEP, reopened_inc.load_rewrites(session.id()).await)?
            .len()
            == 1,
        "adopted rewrites must survive reopen",
    )?;
    Ok(())
}
