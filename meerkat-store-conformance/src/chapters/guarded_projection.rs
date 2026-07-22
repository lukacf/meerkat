//! Guarded-projection profile:
//! `save_authoritative_projection_if_current_revision` semantics and the
//! non-atomic projection-vs-authority recovery contract.

use meerkat_core::session_store::session_projection_cas_token;
use meerkat_core::{Session, SessionStore, SessionStoreError};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "guarded_projection";

/// Guarded-projection profile.
///
/// The projection row is derived state: a separate authority (the runtime
/// store) has already committed the semantic mutation, and the projection
/// write is CAS-guarded against the row the caller already validated. The
/// projection write and the authority commit are **not atomic** — remote
/// backends never have cross-store transactions — so the recovery protocol
/// is the contract, and this chapter pins its quarantine-shaped sequence as
/// a tested contract:
///
/// 1. guarded projection save succeeds;
/// 2. a dependent effect (e.g. the audit-event append) fails;
/// 3. rollback: guarded save of the last audited projection, keyed on the
///    just-written row's token;
/// 4. if rollback itself fails: quarantine via
///    `delete_if_current_revision(id, just_written_token)` so a stale,
///    unaudited projection is never served.
pub async fn guarded_projection(
    factory: &dyn SessionStoreFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    create_when_absent(&steps, store.as_ref()).await?;
    cas_semantics(&steps, store.as_ref()).await?;
    recovery_protocol(&steps, store.as_ref()).await?;
    Ok(())
}

async fn load_current(
    steps: &Steps,
    step: &'static str,
    store: &dyn SessionStore,
    session: &Session,
) -> Result<(Session, String), ConformanceFailure> {
    let current = steps
        .wrap(step, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(step, "projection row must load"))?;
    let token = steps.wrap(step, session_projection_cas_token(&current))?;
    Ok((current, token))
}

async fn create_when_absent(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "create_when_absent";
    let session = fixtures::session_with_texts(&["projected turn"]);

    // expected = None means "no row may exist yet".
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(&session, None)
            .await,
    )?;
    let (loaded, _token) = load_current(steps, STEP, store, &session).await?;
    steps.ensure(
        STEP,
        loaded.messages().len() == 1,
        "guarded create must persist the projection",
    )?;

    // expected = Some(token) against an ABSENT row must be rejected.
    let absent = fixtures::session_with_texts(&["never persisted"]);
    let bogus_token = steps.wrap(STEP, session_projection_cas_token(&absent))?;
    match store
        .save_authoritative_projection_if_current_revision(&absent, Some(bogus_token))
        .await
    {
        Err(SessionStoreError::TranscriptContinuityViolation { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "guarded save expecting a token over an absent row must fail with \
                     TranscriptContinuityViolation, got: {other}"
                ),
            ));
        }
        Ok(()) => {
            return Err(steps.fail(
                STEP,
                "guarded save expecting a token over an absent row must be rejected",
            ));
        }
    }
    Ok(())
}

async fn cas_semantics(steps: &Steps, store: &dyn SessionStore) -> Result<(), ConformanceFailure> {
    const STEP: &str = "cas_semantics";
    let session = fixtures::session_with_texts(&["base"]);
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(&session, None)
            .await,
    )?;

    // Advance with the current row's token.
    let (current, token) = load_current(steps, STEP, store, &session).await?;
    let mut advanced = current;
    fixtures::push_text(&mut advanced, "advanced");
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(&advanced, Some(token.clone()))
            .await,
    )?;

    // Replaying the consumed token must be rejected, and the row unchanged.
    let mut stale = advanced.clone();
    fixtures::push_text(&mut stale, "stale writer");
    match store
        .save_authoritative_projection_if_current_revision(&stale, Some(token))
        .await
    {
        Err(SessionStoreError::TranscriptContinuityViolation { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!(
                    "stale-token guarded save must fail with TranscriptContinuityViolation, \
                     got: {other}"
                ),
            ));
        }
        Ok(()) => return Err(steps.fail(STEP, "stale-token guarded save must be rejected")),
    }
    let (row, _token) = load_current(steps, STEP, store, &session).await?;
    steps.ensure(
        STEP,
        row.messages().len() == advanced.messages().len(),
        "a rejected guarded save must leave the committed row intact",
    )?;
    Ok(())
}

async fn recovery_protocol(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "recovery_protocol";
    let session = fixtures::session_with_texts(&["audited base"]);
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(&session, None)
            .await,
    )?;

    // The last-audited projection: what rollback restores.
    let (audited, audited_token) = load_current(steps, STEP, store, &session).await?;

    // 1. Guarded projection save succeeds...
    let mut rewritten = audited.clone();
    fixtures::push_text(&mut rewritten, "unaudited projection write");
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(
                &rewritten,
                Some(audited_token.clone()),
            )
            .await,
    )?;
    let (_, rewritten_token) = load_current(steps, STEP, store, &session).await?;

    // 2. ...then the dependent audit append fails (simulated), so
    // 3. rollback: guarded save of the last audited projection keyed on the
    //    just-written row's token. Note this restore may SHRINK the row —
    //    the guarded projection path is CAS-trusted, not append-only.
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(
                &audited,
                Some(rewritten_token.clone()),
            )
            .await,
    )?;
    let (restored, _restored_token) = load_current(steps, STEP, store, &session).await?;
    steps.ensure(
        STEP,
        restored.messages().len() == audited.messages().len(),
        "rollback must restore the last audited projection",
    )?;

    // 4. The quarantine arm: when rollback itself is impossible, the caller
    //    deletes the unaudited row if (and only if) it is still current.
    let (current, current_token) = load_current(steps, STEP, store, &session).await?;
    let mut unauditable = current;
    fixtures::push_text(&mut unauditable, "unauditable projection write");
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(&unauditable, Some(current_token))
            .await,
    )?;
    let (_, unaudited_token) = load_current(steps, STEP, store, &session).await?;
    steps.ensure(
        STEP,
        steps.wrap(
            STEP,
            store
                .delete_if_current_revision(session.id(), &unaudited_token)
                .await,
        )?,
        "quarantine delete keyed on the unaudited row's token must succeed",
    )?;
    steps.ensure(
        STEP,
        steps.wrap(STEP, store.load(session.id()).await)?.is_none(),
        "a quarantined projection must not be served",
    )?;
    Ok(())
}
