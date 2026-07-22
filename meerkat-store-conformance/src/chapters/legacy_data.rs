//! Legacy-data chapter: "open a store written by version N−1" as a
//! first-class axis.
//!
//! Fixtures are synthetic 0.7.x-shaped rows: **current-envelope** session
//! JSON whose metadata LACKS the reserved `session_checkpoint_stamp_v1` key,
//! optionally carrying the `session_runtime_checkpoint_provenance_v1`
//! boolean marker. Do NOT fabricate v0/v1 envelopes —
//! `meerkat_core::adopt_legacy_session` rejects them by design.

use meerkat_core::{
    SessionCheckpointMetadataState, SessionCheckpointRevision, SessionCheckpointState,
    SessionGeneration, SessionStore, adopt_legacy_session, session_checkpoint_metadata_state,
};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "legacy_data";

/// Legacy-data chapter: a store loads legacy rows, reports them as
/// `LegacyUnverified` (with the runtime-checkpoint marker preserved),
/// preserves the document on round-trip, and a stamped (adopted) session
/// stays `Verified` across save/load and reopen.
pub async fn legacy_data(factory: &dyn SessionStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    legacy_row_reports_unverified(&steps, store.as_ref()).await?;
    legacy_runtime_marker_preserved(&steps, store.as_ref()).await?;
    legacy_document_round_trip(&steps, store.as_ref()).await?;
    adoption_upgrade_and_survival(&steps, factory, store.as_ref()).await?;
    Ok(())
}

async fn legacy_row_reports_unverified(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "legacy_row_reports_unverified";
    let session = fixtures::session_with_texts(&["legacy turn one", "legacy turn two"]);
    steps.wrap(STEP, store.save(&session).await)?;

    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "legacy row must load"))?;
    match steps.wrap(
        STEP,
        session_checkpoint_metadata_state(loaded.id(), loaded.metadata()),
    )? {
        SessionCheckpointMetadataState::LegacyUnverified {
            legacy_runtime_checkpoint: false,
        } => {}
        SessionCheckpointMetadataState::LegacyUnverified {
            legacy_runtime_checkpoint: true,
        } => {
            return Err(steps.fail(
                STEP,
                "a plain legacy row must not gain the runtime-checkpoint provenance marker",
            ));
        }
        SessionCheckpointMetadataState::Stamped(_) => {
            return Err(steps.fail(
                STEP,
                "an unstamped legacy row must decode LegacyUnverified, never Stamped — the \
                 store must not launder absence into verified authority",
            ));
        }
    }
    match steps.wrap(STEP, loaded.try_checkpoint_state())? {
        SessionCheckpointState::LegacyUnverified { .. } => Ok(()),
        SessionCheckpointState::Verified(_) => Err(steps.fail(
            STEP,
            "an unstamped legacy row must not verify as typed checkpoint authority",
        )),
    }
}

async fn legacy_runtime_marker_preserved(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "legacy_runtime_marker_preserved";
    let session = fixtures::session_with_texts(&["legacy runtime checkpoint"]);
    let marked = fixtures::with_legacy_runtime_checkpoint_marker(&session, true)?;
    steps.wrap(STEP, store.save(&marked).await)?;

    let loaded = steps
        .wrap(STEP, store.load(marked.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "marked legacy row must load"))?;
    match steps.wrap(
        STEP,
        session_checkpoint_metadata_state(loaded.id(), loaded.metadata()),
    )? {
        SessionCheckpointMetadataState::LegacyUnverified {
            legacy_runtime_checkpoint: true,
        } => Ok(()),
        SessionCheckpointMetadataState::LegacyUnverified {
            legacy_runtime_checkpoint: false,
        } => Err(steps.fail(
            STEP,
            "the session_runtime_checkpoint_provenance_v1 marker must survive save/load",
        )),
        SessionCheckpointMetadataState::Stamped(_) => Err(steps.fail(
            STEP,
            "a legacy runtime-checkpoint row must stay LegacyUnverified until explicit adoption",
        )),
    }
}

async fn legacy_document_round_trip(
    steps: &Steps,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "legacy_document_round_trip";
    let session = fixtures::session_with_texts(&["byte fidelity one", "byte fidelity two"]);
    let saved_document = steps.wrap(STEP, serde_json::to_value(&session))?;
    steps.wrap(STEP, store.save(&session).await)?;

    let loaded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "legacy row must load"))?;
    let loaded_document = steps.wrap(STEP, serde_json::to_value(&loaded))?;
    steps.ensure(
        STEP,
        loaded_document == saved_document,
        "a legacy document must round-trip semantically byte-preserving (canonical JSON \
         value equality) — adoption binds the stamp to the exact source bytes, so a store \
         that rewrites legacy documents breaks migration custody",
    )?;
    Ok(())
}

async fn adoption_upgrade_and_survival(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "adoption_upgrade_and_survival";
    let session = fixtures::session_with_texts(&["adoptable turn"]);

    // The migration write shape: the legacy row is already persisted, then
    // the adopted (stamped) document is saved over it — a metadata-only
    // upgrade the append-only guard admits.
    steps.wrap(STEP, store.save(&session).await)?;
    let stored_legacy = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "legacy row must load before adoption"))?;
    let blob = fixtures::legacy_session_blob(&stored_legacy)?;
    let adopted = steps.wrap(
        STEP,
        adopt_legacy_session(
            &blob,
            SessionGeneration::INITIAL,
            SessionCheckpointRevision::INITIAL,
        ),
    )?;
    steps.wrap(STEP, store.save(&adopted.session).await)?;

    let upgraded = steps
        .wrap(STEP, store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "adopted row must load"))?;
    match steps.wrap(STEP, upgraded.try_checkpoint_state())? {
        SessionCheckpointState::Verified(stamp) if stamp == adopted.stamp => {}
        SessionCheckpointState::Verified(_) => {
            return Err(steps.fail(STEP, "the served stamp must equal the adopted stamp"));
        }
        SessionCheckpointState::LegacyUnverified { .. } => {
            return Err(steps.fail(
                STEP,
                "the adopted (stamped) document must verify after the upgrade save",
            ));
        }
    }

    // Restart survival: adoption is one-time; the stamp must survive reopen.
    let reopened = factory.open().await?;
    let survived = steps
        .wrap(STEP, reopened.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "adopted row must survive reopen"))?;
    match steps.wrap(STEP, survived.try_checkpoint_state())? {
        SessionCheckpointState::Verified(stamp) if stamp == adopted.stamp => Ok(()),
        _ => Err(steps.fail(
            STEP,
            "a stamped (adopted) session must stay Verified across reopen",
        )),
    }
}
