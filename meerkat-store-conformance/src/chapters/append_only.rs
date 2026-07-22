//! Append-only media chapter: what a revision guard MEANS for backends that
//! emulate CAS with windowed reads over append-only sibling rows.
//!
//! Pinned contracts (from the ob3 zombie incident):
//!
//! - **Superseded-sibling-row deduplication is the store's job.** A failed or
//!   racing write attempt on append-only media may leave an orphan
//!   higher-version sibling row behind. That orphan must never be adopted as
//!   the current row: reads keyed on the legitimately committed row and
//!   guarded saves keyed on its token must keep succeeding. A store that
//!   resolves "current" as "highest version seen in the window" lets one
//!   failed attempt make every subsequent guarded save permanently stale.
//! - **Checkpoint monotonicity survives generation rebinds.** A persisted
//!   row's checkpoint stamp never regresses: replaying an older stamped row
//!   (e.g. a pre-rebind copy at a lower checkpoint revision) is rejected by
//!   the append-only guard, and nonzero observed generation/revision cursors
//!   are preserved exactly — never normalized back to INITIAL.
//!
//! The in-crate [`EmulatedCasSessionStore`](crate::EmulatedCasSessionStore)
//! passes this chapter and doubles as the documented reference for the
//! contract.

use meerkat_core::session_store::session_projection_cas_token;
use meerkat_core::{
    Session, SessionCheckpointProvenance, SessionCheckpointRevision, SessionCheckpointStamp,
    SessionCheckpointState, SessionGeneration, SessionStore, adopt_legacy_session,
};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "append_only";

/// Number of failed-attempt/committed-write rounds in the zombie pin.
const CONFLICT_ROUNDS: usize = 3;

/// Append-only media chapter. Applies to every `SessionStore` (disk backends
/// with real transactions must satisfy the same observable contract that
/// emulated-CAS backends are held to).
pub async fn append_only(factory: &dyn SessionStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    orphan_siblings_never_wedge_guarded_saves(&steps, store.as_ref()).await?;
    checkpoint_monotonicity_across_generation_rebind(&steps, store.as_ref()).await?;
    Ok(())
}

/// The ob3 zombie pin.
///
/// A committed writer holds the token of its own last committed row. A stale
/// interloper's guarded save fails (on append-only media this typically
/// leaves an orphan higher-version sibling row). The committed writer must
/// then still be able to (a) read back exactly what it committed and
/// (b) advance with a guarded save keyed on its remembered token — round
/// after round, without the orphans accumulating into permanent staleness.
async fn orphan_siblings_never_wedge_guarded_saves(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "orphan_siblings_never_wedge_guarded_saves";
    let base = fixtures::session_with_texts(&["append-only base"]);
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(&base, None)
            .await,
    )?;

    // A stale interloper's baseline: the seed row. It goes genuinely stale
    // after the committed writer's first advance below, and never advances.
    let stale_row = steps
        .wrap(STEP, store.load(base.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "seeded row must load"))?;
    let stale_token = steps.wrap(STEP, session_projection_cas_token(&stale_row))?;

    // The committed writer advances once, consuming the seed token; its
    // knowledge from here on is its own last committed row + token.
    let mut first_advance = stale_row.clone();
    fixtures::push_text(&mut first_advance, "committed first advance");
    steps.wrap(
        STEP,
        store
            .save_authoritative_projection_if_current_revision(
                &first_advance,
                Some(stale_token.clone()),
            )
            .await,
    )?;
    let mut committed_row = steps
        .wrap(STEP, store.load(base.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "advanced row must load"))?;
    let mut committed_token = steps.wrap(STEP, session_projection_cas_token(&committed_row))?;

    for round in 0..CONFLICT_ROUNDS {
        // Interloper: guarded save keyed on the stale token. Must fail; on
        // append-only media the attempt row it inserted is now an orphan
        // higher-version sibling.
        let mut interloper = stale_row.clone();
        fixtures::push_text(&mut interloper, &format!("interloper round {round}"));
        steps.ensure(
            STEP,
            store
                .save_authoritative_projection_if_current_revision(
                    &interloper,
                    Some(stale_token.clone()),
                )
                .await
                .is_err(),
            format!("round {round}: stale-token guarded save must be rejected"),
        )?;

        // The committed writer's read must still serve exactly what it
        // committed — the orphan sibling must not shadow the current row.
        let served = steps
            .wrap(STEP, store.load(base.id()).await)?
            .ok_or_else(|| steps.fail(STEP, "committed row must still load"))?;
        let served_token = steps.wrap(STEP, session_projection_cas_token(&served))?;
        steps.ensure(
            STEP,
            served_token == committed_token,
            format!(
                "round {round}: after a failed guarded save, the store serves a row that is \
                 not the last committed one — an orphan higher-version sibling has become \
                 current (superseded-sibling deduplication is the store's job)"
            ),
        )?;

        // The committed writer advances, keyed on ITS OWN remembered token.
        let mut next = committed_row.clone();
        fixtures::push_text(&mut next, &format!("committed round {round}"));
        steps.wrap(
            STEP,
            store
                .save_authoritative_projection_if_current_revision(
                    &next,
                    Some(committed_token.clone()),
                )
                .await,
        )?;

        committed_row = steps
            .wrap(STEP, store.load(base.id()).await)?
            .ok_or_else(|| steps.fail(STEP, "advanced row must load"))?;
        let expected_revision = steps.wrap(STEP, next.transcript_revision())?;
        let served_revision = steps.wrap(STEP, committed_row.transcript_revision())?;
        steps.ensure(
            STEP,
            served_revision == expected_revision,
            format!("round {round}: the advanced row must be served after commit"),
        )?;
        committed_token = steps.wrap(STEP, session_projection_cas_token(&committed_row))?;
    }

    // The revision-guarded delete obeys the same resolution: stale token
    // no-ops, the committed token deletes.
    steps.ensure(
        STEP,
        !steps.wrap(
            STEP,
            store
                .delete_if_current_revision(base.id(), &stale_token)
                .await,
        )?,
        "stale-token delete_if_current_revision must return false after the conflict rounds",
    )?;
    steps.ensure(
        STEP,
        steps.wrap(
            STEP,
            store
                .delete_if_current_revision(base.id(), &committed_token)
                .await,
        )?,
        "committed-token delete_if_current_revision must succeed after the conflict rounds",
    )?;
    Ok(())
}

/// Checkpoint monotonicity across a generation rebind.
///
/// Adopt a legacy blob at a NONZERO observed cursor (generation 3, revision
/// 4 — the rebind shape: the continuity row recorded a generation floor).
/// The store must preserve the cursor exactly, serve the successor stamp
/// after an advance, and reject a replay of the older stamped row.
async fn checkpoint_monotonicity_across_generation_rebind(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "checkpoint_monotonicity_across_generation_rebind";
    let observed_generation = SessionGeneration::new(3);
    let observed_revision = SessionCheckpointRevision::new(4);

    let session = fixtures::session_with_texts(&["rebind turn one", "rebind turn two"]);
    let blob = fixtures::legacy_session_blob(&session)?;
    let adopted = steps.wrap(
        STEP,
        adopt_legacy_session(&blob, observed_generation, observed_revision),
    )?;
    steps.wrap(STEP, store.save(&adopted.session).await)?;

    // The nonzero cursor is preserved exactly — never normalized to INITIAL.
    let loaded = steps
        .wrap(STEP, store.load(adopted.session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "adopted session must load"))?;
    let stamp = verified_stamp(steps, STEP, &loaded)?;
    steps.ensure(
        STEP,
        stamp.generation() == observed_generation
            && stamp.checkpoint_revision() == observed_revision,
        format!(
            "adopted cursor must be preserved exactly: expected generation {}/revision {}, \
             got generation {}/revision {}",
            observed_generation.get(),
            observed_revision.get(),
            stamp.generation().get(),
            stamp.checkpoint_revision().get()
        ),
    )?;

    // Advance: successor stamp at revision 5 over an appended transcript.
    let mut advanced = loaded;
    fixtures::push_text(&mut advanced, "rebind turn three");
    let successor = steps.wrap(
        STEP,
        SessionCheckpointStamp::successor(
            &advanced,
            &stamp,
            SessionCheckpointProvenance::RunBoundaryCommit,
        ),
    )?;
    steps.wrap(STEP, advanced.install_checkpoint_stamp(successor.clone()))?;
    steps.wrap(STEP, store.save(&advanced).await)?;
    let served = steps
        .wrap(STEP, store.load(advanced.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "advanced session must load"))?;
    let served_stamp = verified_stamp(steps, STEP, &served)?;
    steps.ensure(
        STEP,
        served_stamp == successor,
        "the successor checkpoint stamp must be served after the advance",
    )?;

    // Replaying the older stamped row (shorter transcript, revision 4) must
    // be rejected, keeping the persisted checkpoint monotonic.
    let replay: Session = steps.wrap(STEP, serde_json::from_slice(&adopted.serialized))?;
    steps.ensure(
        STEP,
        store.save(&replay).await.is_err(),
        "replaying an older stamped row must be rejected by the append-only guard",
    )?;
    let after_replay = steps
        .wrap(STEP, store.load(advanced.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "row must still load after the rejected replay"))?;
    let after_stamp = verified_stamp(steps, STEP, &after_replay)?;
    steps.ensure(
        STEP,
        after_stamp.checkpoint_revision() == successor.checkpoint_revision(),
        "the persisted checkpoint revision must never regress",
    )?;
    Ok(())
}

fn verified_stamp(
    steps: &Steps,
    step: &'static str,
    session: &Session,
) -> Result<SessionCheckpointStamp, ConformanceFailure> {
    match steps.wrap(step, session.try_checkpoint_state())? {
        SessionCheckpointState::Verified(stamp) => Ok(stamp),
        SessionCheckpointState::LegacyUnverified { .. } => Err(steps.fail(
            step,
            "expected a verified checkpoint stamp; the store lost the reserved stamp metadata",
        )),
    }
}
