//! Legacy-data chapter: "open a store written by version N−1" as a
//! first-class axis.
//!
//! Driven by **byte-literal** fixture documents
//! ([`fixtures::legacy_v07_session_fixture`]): the serialized 0.7.x shape —
//! current-envelope (`version: 2`) JSON whose metadata LACKS the reserved
//! `session_checkpoint_stamp_v1` key, message content in the legacy
//! plain-string form, optionally carrying the
//! `session_runtime_checkpoint_provenance_v1` boolean marker — is installed
//! into the factory's storage ([`SessionStoreFactory::install_session_document`])
//! and then opened through the current backend. Do NOT fabricate v0/v1
//! envelopes — `meerkat_core::adopt_legacy_session` rejects them by design.

use meerkat_core::{
    Session, SessionCheckpointMetadataState, SessionCheckpointRevision, SessionCheckpointState,
    SessionGeneration, SessionStore, adopt_legacy_session, session_checkpoint_metadata_state,
};

use crate::factory::SessionStoreFactory;
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "legacy_data";

/// Legacy-data chapter: byte-literal N−1 fixture documents decode stably,
/// install into the factory's storage, load through the current backend as
/// `LegacyUnverified` (with the runtime-checkpoint marker preserved),
/// round-trip without being rewritten, and an adopted (stamped) session
/// stays `Verified` across save/load and reopen.
pub async fn legacy_data(factory: &dyn SessionStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);

    fixture_decode_stability(&steps)?;
    let store = factory.open().await?;
    legacy_row_reports_unverified(&steps, factory, store.as_ref()).await?;
    legacy_runtime_marker_preserved(&steps, factory, store.as_ref()).await?;
    legacy_document_round_trip(&steps, factory, store.as_ref()).await?;
    adoption_upgrade_and_survival(&steps, factory, store.as_ref()).await?;
    Ok(())
}

/// Install a fixture and load it back through the current backend.
async fn install_and_load(
    steps: &Steps,
    step: &'static str,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
    fixture: &fixtures::LegacySessionFixture,
) -> Result<Session, ConformanceFailure> {
    factory.install_session_document(&fixture.document).await?;
    steps
        .wrap(step, store.load(&fixture.id).await)?
        .ok_or_else(|| steps.fail(step, "installed legacy fixture row must load"))
}

/// The N−1 fixture bytes must decode through the current wire and re-encode
/// to the identical document. This is a harness-level pin on the legacy
/// shape itself: a drift here breaks migration custody for every store that
/// persisted 0.7.x data, independent of any one backend.
fn fixture_decode_stability(steps: &Steps) -> Result<(), ConformanceFailure> {
    const STEP: &str = "fixture_decode_stability";
    for fixture in [
        fixtures::legacy_v07_session_fixture(),
        fixtures::legacy_v07_runtime_checkpoint_fixture(),
    ] {
        let decoded = fixtures::decode_session_document(&fixture.document)?;
        steps.ensure(
            STEP,
            decoded.id() == &fixture.id,
            "decoded fixture must carry the fixture's session id",
        )?;
        let reencoded = steps.wrap(STEP, serde_json::to_value(&decoded))?;
        let raw: serde_json::Value = steps.wrap(STEP, serde_json::from_slice(&fixture.document))?;
        steps.ensure(
            STEP,
            reencoded == raw,
            "the byte-literal N−1 fixture document must decode/encode stably through the \
             current wire (canonical JSON value equality) — a drifting legacy shape breaks \
             every store that persisted it",
        )?;
    }
    Ok(())
}

async fn legacy_row_reports_unverified(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "legacy_row_reports_unverified";
    let fixture = fixtures::legacy_v07_session_fixture();
    let loaded = install_and_load(steps, STEP, factory, store, &fixture).await?;
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
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "legacy_runtime_marker_preserved";
    let fixture = fixtures::legacy_v07_runtime_checkpoint_fixture();
    let loaded = install_and_load(steps, STEP, factory, store, &fixture).await?;
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
            "the session_runtime_checkpoint_provenance_v1 marker must survive the legacy row",
        )),
        SessionCheckpointMetadataState::Stamped(_) => Err(steps.fail(
            STEP,
            "a legacy runtime-checkpoint row must stay LegacyUnverified until explicit adoption",
        )),
    }
}

async fn legacy_document_round_trip(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "legacy_document_round_trip";
    let fixture = fixtures::legacy_v07_session_fixture();
    let loaded = install_and_load(steps, STEP, factory, store, &fixture).await?;
    let loaded_document = steps.wrap(STEP, serde_json::to_value(&loaded))?;
    let fixture_document: serde_json::Value =
        steps.wrap(STEP, serde_json::from_slice(&fixture.document))?;
    steps.ensure(
        STEP,
        loaded_document == fixture_document,
        "a legacy document must round-trip semantically byte-preserving (canonical JSON \
         value equality against the installed fixture bytes) — adoption binds the stamp to \
         the exact source bytes, so a store that rewrites legacy documents breaks migration \
         custody",
    )?;
    Ok(())
}

async fn adoption_upgrade_and_survival(
    steps: &Steps,
    factory: &dyn SessionStoreFactory,
    store: &dyn SessionStore,
) -> Result<(), ConformanceFailure> {
    const STEP: &str = "adoption_upgrade_and_survival";
    let fixture = fixtures::legacy_v07_session_fixture();

    // The migration write shape: the legacy row is already persisted, then
    // the adopted (stamped) document is saved over it — a metadata-only
    // upgrade the append-only guard admits.
    let stored_legacy = install_and_load(steps, STEP, factory, store, &fixture).await?;
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
        .wrap(STEP, store.load(&fixture.id).await)?
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
        .wrap(STEP, reopened.load(&fixture.id).await)?
        .ok_or_else(|| steps.fail(STEP, "adopted row must survive reopen"))?;
    match steps.wrap(STEP, survived.try_checkpoint_state())? {
        SessionCheckpointState::Verified(stamp) if stamp == adopted.stamp => Ok(()),
        _ => Err(steps.fail(
            STEP,
            "a stamped (adopted) session must stay Verified across reopen",
        )),
    }
}
