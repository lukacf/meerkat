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
/// token, `TranscriptRevisionConflict` on token/parent mismatch,
/// `load_messages` / `load_rewrites` round-trips, reconstruction fidelity
/// across chained rewrites that share no prefix with their parent, and the
/// conditional range-read capability contract (`load_canonical_head` /
/// `load_rewrite_commits`).
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
    chained_prefix_rewrites(&steps, factory, &store, inc.as_ref()).await?;
    range_read_capability(&steps, &store, inc.as_ref()).await?;
    Ok(())
}

/// Ensure the commit view equals `load_rewrites`' commits, in order.
async fn ensure_commit_view_matches(
    steps: &Steps,
    step: &'static str,
    inc: &dyn IncrementalSessionStore,
    id: &meerkat_core::SessionId,
    what: &str,
) -> Result<(), ConformanceFailure> {
    let commits = steps.wrap(step, inc.load_rewrite_commits(id).await)?;
    let derived = steps
        .wrap(step, inc.load_rewrites(id).await)?
        .into_iter()
        .map(|record| record.commit)
        .collect::<Vec<_>>();
    steps.ensure(
        step,
        commits == derived,
        format!("load_rewrite_commits must equal load_rewrites' commits, in order ({what})"),
    )
}

/// Conditional range-read capability pins.
///
/// `load_canonical_head` has a conservative default (`None`), so a store that
/// never advertises a canonical head remains fully conformant — the `Some`
/// pins below apply only when the store opts in. `load_rewrite_commits` must
/// ALWAYS equal `load_rewrites`' commits (the default derives them), so that
/// pin is unconditional.
async fn range_read_capability(
    steps: &Steps,
    store: &Arc<dyn meerkat_core::SessionStore>,
    inc: &dyn IncrementalSessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "range_read_capability";
    let (session, head, token) = seed(steps, STEP, inc, &["range one", "range two"]).await?;

    // Absent session: None, not an error, for both verbs.
    let absent = fixtures::session_with_texts(&["never persisted"])?;
    steps.ensure(
        STEP,
        steps
            .wrap(STEP, inc.load_canonical_head(absent.id()).await)?
            .is_none(),
        "load_canonical_head over an absent session must be None",
    )?;
    steps.ensure(
        STEP,
        steps
            .wrap(STEP, inc.load_rewrite_commits(absent.id()).await)?
            .is_empty(),
        "load_rewrite_commits over an absent session must be empty",
    )?;

    // Unconditional: the commit view equals load_rewrites' commits (empty
    // here — no rewrite yet).
    ensure_commit_view_matches(steps, STEP, inc, session.id(), "fresh head-canonical").await?;

    // Conditional: a store MAY answer None (the conservative default is
    // legal and keeps every reader on the whole-load path). If it answers
    // Some for this head-canonical session, the row must be the persisted
    // head itself and its strand must page-serve exactly.
    if let Some(canonical) = steps.wrap(STEP, inc.load_canonical_head(session.id()).await)? {
        steps.ensure(
            STEP,
            canonical == head,
            "an advertised canonical head must equal the saved head row",
        )?;
        let loaded_head = steps
            .wrap(STEP, inc.load_head(session.id()).await)?
            .ok_or_else(|| steps.fail(STEP, "head-canonical session must load_head"))?;
        steps.ensure(
            STEP,
            canonical == loaded_head,
            "for a head-canonical session the canonical head must equal load_head's row",
        )?;

        // Page-serving: ranges over the canonical head's strand serve
        // exactly the slim transcript's rows.
        let slim = steps
            .wrap(STEP, store.load(session.id()).await)?
            .ok_or_else(|| steps.fail(STEP, "head-canonical session must load"))?;
        let count = canonical.message_count;
        let all = steps.wrap(
            STEP,
            inc.load_messages(session.id(), &canonical.strand, 0..count)
                .await,
        )?;
        steps.ensure(
            STEP,
            all.as_slice() == slim.messages(),
            "the canonical head's strand must serve exactly the head-covered messages",
        )?;
        for start in 0..count {
            for end in start..=count {
                let page = steps.wrap(
                    STEP,
                    inc.load_messages(session.id(), &canonical.strand, start..end)
                        .await,
                )?;
                let expected = &slim.messages()[usize::try_from(start)
                    .map_err(|_| steps.fail(STEP, "message_count exceeds the address space"))?
                    ..usize::try_from(end).map_err(|_| {
                        steps.fail(STEP, "message_count exceeds the address space")
                    })?];
                steps.ensure(
                    STEP,
                    page.as_slice() == expected,
                    format!(
                        "canonical-strand range {start}..{end} must page-serve exactly the \
                         corresponding slim rows"
                    ),
                )?;
            }
        }
    }

    // Rewrite lifecycle: the commit view tracks adoption exactly.
    let mut rewritten = session.clone();
    let commit = steps.wrap(
        STEP,
        rewritten.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::text(
                "[conformance] range-read summary".to_string(),
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
    let mechanical_next = steps.wrap(
        STEP,
        inc.commit_rewrite(
            session.id(),
            &record,
            SessionHeadCas::IfToken(token.clone()),
        )
        .await,
    )?;
    let next = steps.wrap(
        STEP,
        SessionHead::from_session(
            &rewritten,
            mechanical_next.strand,
            mechanical_next.rewrite_count,
        ),
    )?;

    // Recorded but NOT adopted: served by neither view.
    steps.ensure(
        STEP,
        steps
            .wrap(STEP, inc.load_rewrite_commits(session.id()).await)?
            .is_empty(),
        "recorded-but-unadopted commits must not be served by load_rewrite_commits",
    )?;
    ensure_commit_view_matches(steps, STEP, inc, session.id(), "recorded-but-unadopted").await?;

    // Adopted: exactly the commit, and still equal to load_rewrites' view.
    steps.wrap(
        STEP,
        inc.save_head(&next, SessionHeadCas::IfToken(token)).await,
    )?;
    let commits = steps.wrap(STEP, inc.load_rewrite_commits(session.id()).await)?;
    steps.ensure(
        STEP,
        commits.len() == 1 && commits[0] == commit,
        "the adopted commit must be served by load_rewrite_commits",
    )?;
    ensure_commit_view_matches(steps, STEP, inc, session.id(), "adopted").await?;

    // If the store advertises canonical heads, the post-adoption head must
    // be the adopted row and its strand must serve the rewritten transcript.
    if let Some(canonical) = steps.wrap(STEP, inc.load_canonical_head(session.id()).await)? {
        steps.ensure(
            STEP,
            canonical.rewrite_count == 1 && canonical.head_revision == commit.revision,
            "the post-adoption canonical head must carry the adopted commit revision",
        )?;
        let page = steps.wrap(
            STEP,
            inc.load_messages(session.id(), &canonical.strand, 0..canonical.message_count)
                .await,
        )?;
        steps.ensure(
            STEP,
            page.as_slice() == rewritten.messages(),
            "the post-adoption canonical strand must serve exactly the rewritten transcript",
        )?;
    }
    Ok(())
}

/// Chained rewrites that share NO prefix with the transcript they replace.
///
/// Every round here replaces message 0 and preserves every later message.
/// Prefix addressing buys a backend nothing on this deliberately adversarial
/// rewrite shape, so a durable store must express the superseded transcript
/// as a delta of its successor. The contract this pins is reconstruction, not
/// layout: after N chained rewrites, every retained body must still come back
/// EXACTLY, the live transcript must still be the last revision, and all of it
/// must survive reopen. A backend that keeps whole revisions materialized
/// passes too — this chapter refuses to encode any one storage strategy, only
/// the fidelity every strategy owes.
async fn chained_prefix_rewrites(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &Arc<dyn meerkat_core::SessionStore>,
    inc: &dyn IncrementalSessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "chained_prefix_rewrites";
    const ROUNDS: usize = 4;
    let (session, _head, mut token) =
        seed(steps, STEP, inc, &["opening", "second", "third"]).await?;

    let mut live = session.clone();
    let mut expected: Vec<(Vec<Message>, Vec<Message>)> = Vec::new();
    for round in 0..ROUNDS {
        let parent_messages = live.messages().to_vec();
        let commit = steps.wrap(
            STEP,
            live.commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![Message::User(UserMessage::text(format!(
                    "[conformance] refreshed opening {round}"
                )))],
                TranscriptRewriteReason::new("conformance"),
                Some("meerkat-store-conformance".to_string()),
                None,
            ),
        )?;
        let parent_body = steps
            .wrap(STEP, live.transcript_revision_body(&commit.parent_revision))?
            .ok_or_else(|| steps.fail(STEP, "rewrite must retain the parent revision body"))?;
        let revision_body = steps
            .wrap(STEP, live.transcript_revision_body(&commit.revision))?
            .ok_or_else(|| steps.fail(STEP, "rewrite must retain the new revision body"))?;
        let record = steps.wrap(
            STEP,
            TranscriptRewriteRecord::new(commit, parent_body, revision_body),
        )?;
        let mechanical_next = steps.wrap(
            STEP,
            inc.commit_rewrite(
                session.id(),
                &record,
                SessionHeadCas::IfToken(token.clone()),
            )
            .await,
        )?;
        let next = steps.wrap(
            STEP,
            SessionHead::from_session(&live, mechanical_next.strand, mechanical_next.rewrite_count),
        )?;
        steps.wrap(
            STEP,
            inc.save_head(&next, SessionHeadCas::IfToken(token)).await,
        )?;
        token = steps.wrap(STEP, session_head_cas_token(&next))?;
        expected.push((parent_messages, live.messages().to_vec()));
    }

    let records = steps.wrap(STEP, inc.load_rewrites(session.id()).await)?;
    steps.ensure(
        STEP,
        records.len() == ROUNDS,
        format!(
            "all {ROUNDS} adopted rewrites must be served, got {}",
            records.len()
        ),
    )?;
    for (index, (record, (parent_messages, revision_messages))) in
        records.iter().zip(&expected).enumerate()
    {
        steps.ensure(
            STEP,
            &record.parent_body.messages == parent_messages,
            format!("chained rewrite {index} must reconstruct its parent body exactly"),
        )?;
        steps.ensure(
            STEP,
            &record.revision_body.messages == revision_messages,
            format!("chained rewrite {index} must reconstruct its revision body exactly"),
        )?;
    }
    ensure_commit_view_matches(steps, STEP, inc, session.id(), "chained prefix rewrites").await?;

    let slim = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "chained-rewrite session must load"))?;
    steps.ensure(
        STEP,
        slim.messages() == live.messages(),
        "the slim load must serve exactly the last chained revision",
    )?;

    // Reopen: whatever a backend rewrote its representation into has to read
    // back the same way from a cold handle.
    let reopened = factory.open().await?;
    let reopened_inc = Arc::clone(&reopened).as_incremental().ok_or_else(|| {
        steps.fail(
            STEP,
            "reopened handle must still expose the incremental capability",
        )
    })?;
    let survived = steps.wrap(STEP, reopened_inc.load_rewrites(session.id()).await)?;
    steps.ensure(
        STEP,
        survived.len() == records.len(),
        "every chained rewrite must survive reopen",
    )?;
    for (index, (after, before)) in survived.iter().zip(&records).enumerate() {
        steps.ensure(
            STEP,
            after.commit == before.commit
                && after.parent_body.messages == before.parent_body.messages
                && after.revision_body.messages == before.revision_body.messages,
            format!("chained rewrite {index} must reconstruct identically after reopen"),
        )?;
    }
    let survived_slim = steps
        .wrap(STEP, reopened.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "chained-rewrite session must load after reopen"))?;
    steps.ensure(
        STEP,
        survived_slim.messages() == live.messages(),
        "the reopened slim load must serve exactly the last chained revision",
    )
}

/// Seed one session through the incremental write path and return
/// `(session, head, head_token)` as stored.
async fn seed(
    steps: &Steps,
    step: &'static str,
    inc: &dyn IncrementalSessionStore,
    texts: &[&str],
) -> Result<(Session, SessionHead, String), ConformanceFailure> {
    let session = fixtures::session_with_texts(texts)?;
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

    // The compat read path serves the slim materialization of the head —
    // exactly the head-covered messages, in order.
    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "incremental session must load via SessionStore::load"))?;
    steps.ensure(
        STEP,
        loaded.messages() == session.messages(),
        "slim load must serve exactly the head-covered messages, in order",
    )?;

    // load_messages round-trips: full range and sub-range serve the exact
    // ordered rows, not merely the right count.
    let root = TranscriptStrandId::root();
    let all = steps.wrap(STEP, inc.load_messages(session.id(), &root, 0..2).await)?;
    steps.ensure(
        STEP,
        all.as_slice() == session.messages(),
        "full-range load_messages must serve both rows in append order",
    )?;
    let tail = steps.wrap(STEP, inc.load_messages(session.id(), &root, 1..2).await)?;
    steps.ensure(
        STEP,
        tail.as_slice() == &session.messages()[1..2],
        "sub-range load_messages must serve exactly the addressed row",
    )?;
    let head_row = steps.wrap(STEP, inc.load_messages(session.id(), &root, 0..1).await)?;
    steps.ensure(
        STEP,
        head_row.as_slice() == &session.messages()[0..1],
        "sub-range load_messages must serve exactly the first row",
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
    fixtures::push_text(&mut session, "three")?;
    let delta = &session.messages()[2..];
    steps.wrap(
        STEP,
        inc.append_messages(session.id(), &root, 2, delta).await,
    )?;

    // Visibility boundary: an appended-but-unadopted row is invisible until
    // the head advances — the head row is the only adoption authority.
    let pre_adoption_head = steps
        .wrap(STEP, inc.load_head(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "head must load before adoption"))?;
    steps.ensure(
        STEP,
        pre_adoption_head.message_count == 2,
        "an appended-but-unadopted strand row must not advance the served head",
    )?;
    let pre_adoption = steps.wrap(STEP, inc.load_messages(session.id(), &root, 0..3).await)?;
    steps.ensure(
        STEP,
        pre_adoption.as_slice() == session.messages(),
        "strand reads may address the appended row before adoption (rows are durable at append)",
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
    let adopted_head = steps
        .wrap(STEP, inc.load_head(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "head must load after adoption"))?;
    steps.ensure(
        STEP,
        adopted_head.message_count == 3 && adopted_head.head_revision == new_head.head_revision,
        "the adopted head must cover the appended row",
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
    fixtures::push_text(&mut session, "three")?;
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
    fixtures::push_text(&mut session, "four")?;
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
    let mechanical_next = steps.wrap(
        STEP,
        inc.commit_rewrite(
            session.id(),
            &record,
            SessionHeadCas::IfToken(token.clone()),
        )
        .await,
    )?;
    let next = steps.wrap(
        STEP,
        SessionHead::from_session(
            &rewritten,
            mechanical_next.strand,
            mechanical_next.rewrite_count,
        ),
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

    // The compat read now serves the compacted (slim) transcript — exactly
    // the commit's revision content, not merely the right count.
    let slim = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "rewritten session must load"))?;
    steps.ensure(
        STEP,
        slim.messages() == rewritten.messages(),
        "post-adoption load must serve exactly the rewritten transcript",
    )?;
    let slim_revision = steps.wrap(
        STEP,
        meerkat_core::transcript_messages_digest(slim.messages()),
    )?;
    steps.ensure(
        STEP,
        slim_revision == commit.revision,
        "the post-adoption transcript must digest to the adopted commit revision",
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
        survived_head.rewrite_count == 1
            && survived_head.message_count == 1
            && survived_head.head_revision == commit.revision,
        "reopened head must carry the adopted rewrite, compacted count, and commit revision",
    )?;
    let survived_rewrites = steps.wrap(STEP, reopened_inc.load_rewrites(session.id()).await)?;
    steps.ensure(
        STEP,
        survived_rewrites.len() == 1 && survived_rewrites[0].commit.revision == commit.revision,
        "the adopted rewrite must survive reopen with its commit identity intact",
    )?;
    let survived_slim = steps
        .wrap(STEP, reopened.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "rewritten session must load after reopen"))?;
    steps.ensure(
        STEP,
        survived_slim.messages() == rewritten.messages(),
        "the reopened slim load must serve exactly the rewritten transcript",
    )?;
    Ok(())
}
