#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
// The H-app staging module loads the H-local staging module for its own
// use, and this target loads it directly as well.
#![allow(clippy::duplicate_mod)]
//! The `storage-migrate-native` census over real 0.9.3 gateway state
//! directories. The fixtures were written by the published 0.9.3 gateway
//! binary (see `tests/fixtures/meta_093_gateway_states/README.md` and the
//! `released_0_9_3_h_local` entry of `tests/fixtures/README.md`); every run
//! here works on an unpacked copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat::SessionStore;
use meerkat_meta::MigrateMode;
use meerkat_meta::identity_first::{
    ContinuitySessionStoreAdapter, ContinuityStore, LocalContinuityStore, SessionRuntimeState,
};
use meerkat_meta::storage_migrate_native::{
    CensusMobStore, FirstBootRowPlan, GatewayStateShape, IdentityCensusClass,
    IdentityCensusRefusal, IdentityCensusRow, IdentityCensusStatus, ImportedActivation,
    ImportedCrossing, ImportedRow, NativeLayoutStatus, NativeMigrateRefusal, NativeMigrateReport,
    NativeMigrateRequest, SessionCensusClass, SessionCensusRow, identity_census_offline,
    migrate_state_dir_native, native_layout_status,
};
use meerkat_runtime::RuntimeStore;
use sha2::{Digest, Sha256};

/// The released 0.9.3 H-local captures (tests/fixtures/released_0_9_3_h_local),
/// staged the one way every consumer stages them.
#[path = "support/released_0_9_3_h_local.rs"]
mod released_0_9_3_h_local;

/// The released 0.9.3 H-app captures (tests/fixtures/released_0_9_3_h_app):
/// state directories of an application that embeds the runtime builder.
#[expect(
    unused_imports,
    reason = "the capture test re-exports the checksum helper; this target does not use it"
)]
#[path = "support/released_0_9_3_h_app.rs"]
mod released_0_9_3_h_app;

fn unpack(shape: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("scratch dir");
    unpack_into(shape, dir.path());
    dir
}

fn unpack_into(shape: &str, dest: &Path) {
    let archive = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/meta_093_gateway_states")
        .join(format!("{shape}.tar.gz"));
    let file = std::fs::File::open(&archive)
        .unwrap_or_else(|error| panic!("{}: {error}", archive.display()));
    tar::Archive::new(flate2::read::GzDecoder::new(file))
        .unpack(dest)
        .unwrap_or_else(|error| panic!("unpack {shape}: {error}"));
}

async fn census(state_dir: &Path) -> NativeMigrateReport {
    migrate_state_dir_native(&NativeMigrateRequest::new(state_dir, MigrateMode::DryRun)).await
}

async fn census_naming(state_dir: &Path, mob_store: &Path) -> NativeMigrateReport {
    migrate_state_dir_native(
        &NativeMigrateRequest::new(state_dir, MigrateMode::DryRun).with_mob_store(mob_store),
    )
    .await
}

/// Edit one database of an unpacked directory, its write-ahead log included.
fn edit(state_dir: &Path, file: &str, sql: &str) {
    let conn = rusqlite::Connection::open(state_dir.join(file)).expect("open fixture database");
    conn.execute_batch(sql).expect("edit fixture database");
}

/// The session a continuity record binds to `identity`.
fn session_of(state_dir: &Path, identity: &str) -> String {
    let conn = rusqlite::Connection::open(state_dir.join("continuity.sqlite3"))
        .expect("open continuity database");
    conn.query_row(
        "SELECT session_id FROM continuity_records WHERE identity = ?1",
        [identity],
        |row| row.get(0),
    )
    .expect("the identity's binding")
}

fn row_of<'a>(report: &'a NativeMigrateReport, session_id: &str) -> &'a SessionCensusRow {
    report
        .sessions
        .iter()
        .find(|row| row.session_id == session_id)
        .unwrap_or_else(|| panic!("no row for {session_id}: {report:#?}"))
}

fn class_counts(report: &NativeMigrateReport) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for row in &report.sessions {
        *counts.entry(format!("{:?}", row.class)).or_insert(0) += 1;
    }
    counts
}

/// The detail of the one row of `class`, which names the proof that
/// classified it.
fn detail_of(report: &NativeMigrateReport, class: SessionCensusClass) -> &str {
    let rows: Vec<_> = report
        .sessions
        .iter()
        .filter(|row| row.class == class)
        .collect();
    assert_eq!(rows.len(), 1, "{report:#?}");
    &rows[0].detail
}

fn counts(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
    pairs
        .iter()
        .map(|(class, count)| ((*class).to_owned(), *count))
        .collect()
}

/// Every row's verdict follows its class: an admitted row names no action, a
/// refused row names one, and only the import and the admitted suffix rows
/// carry an apply effect.
fn assert_row_contract(report: &NativeMigrateReport) {
    for row in &report.sessions {
        assert_eq!(row.admitted, row.class.admitted(), "{row:#?}");
        assert_eq!(row.action.is_some(), !row.admitted, "{row:#?}");
        assert_eq!(
            row.apply_effect.is_some(),
            matches!(
                row.class,
                SessionCensusClass::RuntimeMissing
                    | SessionCensusClass::ProjectionSuffixTerminal
                    | SessionCensusClass::ProjectionSuffixUnbound
            ),
            "{row:#?}"
        );
    }
}

/// The session refusal count is the only whole-run refusal a census of a
/// readable directory reports: every identity of these captures compares
/// clean against the mob storage.
fn assert_only_census_refusals(report: &NativeMigrateReport, refused_sessions: usize) {
    assert_census_refusals_disagreeing(report, refused_sessions, &[]);
}

/// [`assert_only_census_refusals`] for a capture whose edit leaves each
/// identity in `disagreeing` disagreeing with the mob storage: a live
/// member that no record names (U3: 0.9.3 retires before it deletes), or a
/// record naming a session Core last bound to another identity (R4). The
/// identity census refuses exactly those identities, and nothing else.
fn assert_census_refusals_disagreeing(
    report: &NativeMigrateReport,
    refused_sessions: usize,
    disagreeing: &[&str],
) {
    assert_row_contract(report);
    let mut expected = Vec::new();
    if refused_sessions != 0 {
        expected.push(format!(
            "{:?}",
            NativeMigrateRefusal::SessionsRefused {
                count: refused_sessions
            }
        ));
    }
    if !disagreeing.is_empty() {
        expected.push(format!(
            "{:?}",
            NativeMigrateRefusal::IdentitiesRefused {
                count: disagreeing.len()
            }
        ));
    }
    // A sessions-projection layout keeps no identity binding.
    let sessions_only = report.shape == Some(GatewayStateShape::SessionsProjection);
    let actual: Vec<String> = report
        .refusals
        .iter()
        .map(|refusal| format!("{refusal:?}"))
        .collect();
    assert_eq!(actual, expected, "report: {report:#?}");
    assert_eq!(
        report.identity_census,
        if sessions_only {
            IdentityCensusStatus::NoIdentities
        } else {
            IdentityCensusStatus::Ran
        }
    );
    for row in &report.identities {
        if disagreeing.contains(&row.identity.as_str()) {
            assert_eq!(
                row.class,
                IdentityCensusClass::IdentityDisagrees,
                "{row:#?}"
            );
            assert!(!row.admitted, "{row:#?}");
        } else {
            assert!(row.admitted, "{row:#?}");
        }
        assert_eq!(row.action.is_some(), !row.admitted, "{row:#?}");
    }
    for identity in disagreeing {
        assert!(
            report
                .identities
                .iter()
                .any(|row| row.identity == *identity),
            "no identity row for {identity}: {report:#?}"
        );
    }
}

/// Every file of the directory with its sha256, skipping what reading it
/// leaves without changing a database: the maintenance fence's lock files,
/// the empty presence locks Meerkat Core keeps beside a database it opens, and
/// the shared-memory index and empty write-ahead log that a read-only open
/// of a WAL database creates when they are absent (the startup predicate
/// reads the live directory that way).
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut files = BTreeMap::new();
    for entry in walk(dir) {
        let name = entry
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let empty = std::fs::metadata(&entry).is_ok_and(|meta| meta.len() == 0);
        if name.ends_with(".mfence")
            || (name.ends_with(".mpresence") && empty)
            || name.ends_with("-shm")
            || (name.ends_with("-wal") && empty)
        {
            continue;
        }
        let bytes = std::fs::read(&entry).expect("read fixture file");
        files.insert(
            entry.strip_prefix(dir).expect("inside dir").to_path_buf(),
            format!("{:x}", Sha256::digest(&bytes)),
        );
    }
    files
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

#[tokio::test]
async fn sessions_projection_at_rest_is_equal_everywhere() {
    let dir = unpack("s1b-plain");
    let report = census(dir.path()).await;
    assert_eq!(report.shape, Some(GatewayStateShape::SessionsProjection));
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 3)]),
        "{report:#?}"
    );
    assert!(report.sessions.iter().all(|row| row.admitted));
    assert_only_census_refusals(&report, 0);
}

#[tokio::test]
async fn a_lagging_sessions_projection_is_behind_and_admitted() {
    let dir = unpack("s1b-projection-behind");
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 1), ("ProjectionBehind", 1)]),
        "{report:#?}"
    );
    assert!(report.sessions.iter().all(|row| row.admitted));
    // alpha compacted in boot 1 and appended in boot 2: the head row alone
    // proves the projection is the committed prefix past the rewrite.
    assert_eq!(
        detail_of(&report, SessionCensusClass::ProjectionBehind),
        "the projection head is the committed transcript's prefix past its last rewrite"
    );
    assert_only_census_refusals(&report, 0);
}

/// `runtime.sqlite` knows nothing of the session only the projection holds:
/// the one shape 0.9.3 re-mints from the projection, so it is admitted and
/// `--apply` imports it.
#[tokio::test]
async fn a_session_only_the_sessions_projection_holds_is_admitted_for_import() {
    let dir = unpack("s1b-runtime-missing");
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2), ("RuntimeMissing", 1)]),
        "{report:#?}"
    );
    assert_imported_row(&report);
    assert_only_census_refusals(&report, 0);
}

fn assert_imported_row(report: &NativeMigrateReport) {
    let missing = report
        .sessions
        .iter()
        .find(|row| row.class == SessionCensusClass::RuntimeMissing)
        .expect("the runtime-missing row");
    assert!(missing.admitted, "{missing:#?}");
    assert!(
        missing
            .apply_effect
            .as_deref()
            .is_some_and(|effect| effect.contains("imports the projection copy")),
        "{missing:#?}"
    );
    assert!(
        missing.detail.contains("is the seed 0.9.3 re-mints"),
        "{missing:#?}"
    );
}

#[tokio::test]
async fn roster_layout_at_rest_is_equal_and_names_each_identity() {
    let dir = unpack("s1-roster");
    let report = census(dir.path()).await;
    assert_eq!(report.shape, Some(GatewayStateShape::ContinuityProjection));
    assert_eq!(
        report.mob_storage,
        Some(CensusMobStore::Durable {
            path: dir.path().join("event_log.sqlite3")
        })
    );
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 3)]),
        "{report:#?}"
    );
    assert!(
        report.sessions.iter().all(|row| row.identities.len() == 1),
        "every session is bound to one identity: {report:#?}"
    );
    assert_only_census_refusals(&report, 0);
}

#[tokio::test]
async fn a_lagging_continuity_projection_is_behind_and_admitted() {
    let dir = unpack("s1-projection-behind");
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 1), ("ProjectionBehind", 1)]),
        "{report:#?}"
    );
    assert_eq!(
        detail_of(&report, SessionCensusClass::ProjectionBehind),
        "the projection head is the committed transcript's prefix past its last rewrite"
    );
    assert_only_census_refusals(&report, 0);
}

#[tokio::test]
async fn a_session_only_continuity_holds_is_admitted_for_import() {
    let dir = unpack("s1-runtime-missing");
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2), ("RuntimeMissing", 1)]),
        "{report:#?}"
    );
    assert_imported_row(&report);
    assert_only_census_refusals(&report, 0);
}

#[tokio::test]
async fn in_memory_mob_storage_is_its_own_shape() {
    let dir = unpack("s1e-memory-mob");
    let report = census(dir.path()).await;
    assert_eq!(
        report.shape,
        Some(GatewayStateShape::ContinuityProjectionInMemoryMob)
    );
    assert_eq!(report.mob_storage, Some(CensusMobStore::InMemory));
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 3)]),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
}

/// `meta/compact_member` failed in 0.9.3 (stale checkpoint version), marked
/// the identity broken, and the gateway was killed: nothing was committed
/// past what both copies hold, so both sessions read Equal from the files the
/// kill left (the continuity data still in its write-ahead log).
#[tokio::test]
async fn a_failed_member_compaction_and_a_kill_leave_both_copies_equal() {
    let dir = unpack("s1-compact-member-broken-killed");
    let report = census(dir.path()).await;
    assert_eq!(report.shape, Some(GatewayStateShape::ContinuityProjection));
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2)]),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
}

#[tokio::test]
async fn the_census_changes_no_file_of_the_state_directory() {
    for shape in ["s1b-plain", "s1-roster", "s1-compact-member-broken-killed"] {
        let dir = unpack(shape);
        let before = snapshot(dir.path());
        let _report = census(dir.path()).await;
        assert_eq!(snapshot(dir.path()), before, "{shape} changed");
    }
}

/// `--apply` carries out a clean sessions-projection directory:
/// runtime.sqlite converted in place, the projection replaced by its retired
/// stamp, both backed up first, and the startup predicate reads the
/// directory as native. A second run has nothing to migrate.
#[tokio::test]
async fn an_applied_sessions_projection_directory_reads_native() {
    let dir = unpack("s1b-plain");
    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    let applied = report.applied.expect("applied");
    assert!(!applied.resumed);
    assert_eq!(applied.retired, vec!["sessions.sqlite3".to_owned()]);
    assert_eq!(applied.backups.len(), 2, "{applied:#?}");
    assert!(
        applied.backups.iter().all(|path| path.is_file()),
        "{applied:#?}"
    );
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);

    let again =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(again.refusals.is_empty(), "{again:#?}");
    assert!(again.applied.is_none());
    assert_eq!(again.layout, Some(NativeLayoutStatus::Native));
}

/// A session only the projection holds is imported through I1-I4: Core's
/// session store creates its row, crosses it into head-canonical authority
/// and the runtime store activates it. The imported transcript is the
/// projection's, and the directory then reads native.
#[tokio::test]
async fn apply_imports_a_session_only_the_projection_holds() {
    let dir = unpack("s1b-runtime-missing");
    let source = unpack("s1b-runtime-missing");
    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    let missing: Vec<&str> = report
        .sessions
        .iter()
        .filter(|row| row.class == SessionCensusClass::RuntimeMissing)
        .map(|row| row.session_id.as_str())
        .collect();
    assert_eq!(missing.len(), 1, "{report:#?}");
    let applied = report.applied.as_ref().expect("applied");
    assert_eq!(applied.imported.len(), 1, "{applied:#?}");
    let import = &applied.imported[0];
    assert_eq!(import.session_id, missing[0]);
    assert_eq!(import.row, ImportedRow::Created);
    assert_eq!(import.crossing, ImportedCrossing::Converted);
    assert_eq!(import.activation, ImportedActivation::Installed);
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    // A runtime lock file is the import's own; the run leaves no journal.
    assert!(
        !dir.path()
            .join("storage-migrate-native.journal.json")
            .exists()
    );

    let id = meerkat_core::types::SessionId::parse(missing[0]).expect("session id");
    let projection =
        meerkat_store::SqliteSessionStore::open(source.path().join("sessions.sqlite3"))
            .expect("projection");
    let expected = projection
        .load(&id)
        .await
        .expect("load")
        .expect("projection copy");
    let native = meerkat_store::SqliteSessionStore::open(dir.path().join("runtime.sqlite"))
        .expect("native session store");
    let imported = native.load(&id).await.expect("load").expect("imported row");
    assert_eq!(
        meerkat_core::transcript_messages_digest(imported.messages()).expect("digest"),
        meerkat_core::transcript_messages_digest(expected.messages()).expect("digest"),
    );
    assert_eq!(imported.messages().len(), expected.messages().len());

    let again =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(again.refusals.is_empty(), "{again:#?}");
    assert!(again.applied.is_none());
}

/// A lone legacy spelling of the continuity projection (a host's
/// `continuity.db`) is read in place: the session only it holds is imported
/// from that file's own backup, the file keeps its records without session
/// rows, and the directory reads native.
#[tokio::test]
async fn a_legacy_spelled_projection_imports_from_its_own_backup() {
    let dir = unpack("s1-runtime-missing");
    for suffix in ["", "-wal"] {
        let canonical = dir.path().join(format!("continuity.sqlite3{suffix}"));
        if canonical.exists() {
            std::fs::rename(
                &canonical,
                dir.path().join(format!("continuity.db{suffix}")),
            )
            .expect("rename to the legacy spelling");
        }
    }
    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    let applied = report.applied.as_ref().expect("applied");
    assert_eq!(applied.imported.len(), 1, "{applied:#?}");
    assert_eq!(applied.retired, vec!["continuity.db".to_owned()]);
    assert!(dir.path().join("continuity.db").is_file());
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
}

/// A Meta runtime lock file that a stopped run left behind belongs to the
/// run: the directory hold locks it before any step, and the import runs
/// under that lock instead of refusing its own second lock on the file.
#[tokio::test]
async fn an_import_runs_under_the_lock_a_stopped_run_left_behind() {
    let dir = unpack("s1b-runtime-missing");
    std::fs::write(dir.path().join("meta-runtime.lock"), b"").expect("lock file");
    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    assert_eq!(
        report.applied.as_ref().expect("applied").imported.len(),
        1,
        "{report:#?}"
    );
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
}

/// A roster layout over durable mob storage: every identity's continuity
/// record agrees with Core's last binding (the mob keys members by the
/// roster encoding of the identity), so the identity census admits each one
/// with a first-boot Resume row on its session, and `--apply` carries out
/// the directory.
#[tokio::test]
async fn an_identity_layout_applies_once_its_identities_agree() {
    let dir = unpack("s1-roster");
    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    assert_eq!(report.identity_census, IdentityCensusStatus::Ran);
    let identities: Vec<(&str, IdentityCensusClass)> = report
        .identities
        .iter()
        .map(|row| (row.identity.as_str(), row.class))
        .collect();
    assert_eq!(
        identities,
        [
            ("agent:k1-alpha", IdentityCensusClass::Bound),
            ("agent:k1-beta", IdentityCensusClass::Bound),
            ("agent:k1-gamma", IdentityCensusClass::Bound),
        ],
        "{report:#?}"
    );
    assert_eq!(report.first_boot.len(), 3, "{report:#?}");
    for row in &report.first_boot {
        let FirstBootRowPlan::Resume { identity, session } = row else {
            panic!("a bound identity resumes: {row:?}");
        };
        assert!(
            report
                .identities
                .iter()
                .any(|census| &census.identity == identity
                    && census.session_id.as_deref() == Some(session.as_str())),
            "{row:?}"
        );
    }
    let applied = report.applied.as_ref().expect("applied");
    assert_eq!(applied.retired, vec!["continuity.sqlite3".to_owned()]);
    let manifest = meerkat_meta::identity_first::FirstBootManifest::read(dir.path())
        .expect("manifest reads")
        .expect("manifest written");
    assert_eq!(
        applied.first_boot_manifest_sha256.as_deref(),
        Some(manifest.rows_sha256())
    );
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
}

/// In-memory mob storage holds no binding to compare: each bound identity
/// gets a first-boot Resume row on its session, and `--apply` writes them as
/// the first-boot manifest, with a byte copy kept beside it.
#[tokio::test]
async fn in_memory_mob_storage_resumes_each_identity_through_the_manifest() {
    let dir = unpack("s1e-memory-mob");
    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    assert_eq!(report.mob_storage, Some(CensusMobStore::InMemory));
    assert!(
        report
            .identities
            .iter()
            .all(|row| row.class == IdentityCensusClass::Resume),
        "{report:#?}"
    );
    assert!(!report.first_boot.is_empty(), "{report:#?}");
    for row in &report.first_boot {
        let FirstBootRowPlan::Resume { identity, session } = row else {
            panic!("an in-memory row resumes: {row:?}");
        };
        assert!(
            report
                .identities
                .iter()
                .any(|census| &census.identity == identity
                    && census.session_id.as_deref() == Some(session.as_str())),
            "{row:?}"
        );
    }
    let manifest = meerkat_meta::identity_first::FirstBootManifest::read(dir.path())
        .expect("manifest reads")
        .expect("manifest written");
    assert_eq!(manifest.rows().len(), report.first_boot.len());
    let applied = report.applied.as_ref().expect("applied");
    assert_eq!(
        applied.first_boot_manifest_sha256.as_deref(),
        Some(manifest.rows_sha256())
    );
    let copies: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("storage-migrate-native.first-boot.json.pre-")
            })
        })
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    let copy = &copies[0];
    assert_eq!(
        std::fs::read(copy).expect("copy"),
        std::fs::read(dir.path().join("storage-migrate-native.first-boot.json")).expect("manifest")
    );
    let name = copy
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(
        meerkat_meta::storage_migrate::is_registered_backup_artifact_name(&name),
        "{name}"
    );
    assert!(applied.backups.contains(copy), "{applied:#?}");
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
}

/// Both projections hold rows a release reads: the continuity projection's
/// session bodies, and in `sessions.sqlite3` a session row, or a row of
/// another table the retirement would lose. Nothing says which copy 0.9.3
/// served, so the census refuses the directory as ambiguous.
#[tokio::test]
async fn both_projections_present_is_ambiguous() {
    for rows in [
        "CREATE TABLE sessions (session_id TEXT); INSERT INTO sessions VALUES ('s');",
        "CREATE TABLE runtime_inputs (input_id TEXT); INSERT INTO runtime_inputs VALUES ('i');",
    ] {
        let dir = unpack("s1-roster");
        edit(dir.path(), "sessions.sqlite3", rows);
        let report = census(dir.path()).await;
        assert_eq!(report.shape, None, "{rows}: {report:#?}");
        assert!(
            matches!(
                report.refusals.as_slice(),
                [NativeMigrateRefusal::AmbiguousProjection]
            ),
            "{rows}: {report:#?}"
        );
        assert!(report.sessions.is_empty(), "{rows}: {report:#?}");
    }
}

/// A `sessions.sqlite3` as a 0.9.3 host can leave it beside its continuity
/// projection: an empty session store with an empty runtime table, whose
/// only rows are in the schedule tables no release reads since Meta 0.9.0
/// keeps its schedules in `schedule.sqlite`.
fn write_inert_sessions_database(state_dir: &Path) {
    drop(
        meerkat_store::SqliteSessionStore::open(state_dir.join("sessions.sqlite3"))
            .expect("an empty session store"),
    );
    edit(
        state_dir,
        "sessions.sqlite3",
        "CREATE TABLE runtime_inputs (input_id TEXT PRIMARY KEY); \
         CREATE TABLE schedule_schedules (schedule_id TEXT PRIMARY KEY, schedule_json TEXT); \
         CREATE TABLE schedule_occurrences (occurrence_id TEXT PRIMARY KEY, occurrence_json TEXT); \
         CREATE TABLE schedule_receipts (receipt_id TEXT PRIMARY KEY, receipt_json TEXT); \
         INSERT INTO schedule_schedules VALUES ('s1', '{}'), ('s2', '{}'); \
         INSERT INTO schedule_occurrences VALUES ('o1', '{}'), ('o2', '{}'), ('o3', '{}'); \
         INSERT INTO schedule_receipts VALUES ('r1', '{}');",
    );
}

/// A `sessions.sqlite3` that holds no session and nothing else a release
/// reads is no second projection: the continuity projection is the one. The
/// census admits the directory as the continuity layout, and `--apply`
/// retires the empty file with its stamp, so 0.9.4 starts on the directory.
/// Its schedule rows stay in its pre-apply backup.
#[tokio::test]
async fn an_inert_sessions_database_beside_the_continuity_projection_is_retired() {
    let dir = unpack("s1-roster");
    write_inert_sessions_database(dir.path());
    let report = census(dir.path()).await;
    assert_eq!(
        report.shape,
        Some(GatewayStateShape::ContinuityProjection),
        "{report:#?}"
    );
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 3)]),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);

    let report =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    let applied = report.applied.as_ref().expect("applied");
    assert_eq!(
        applied.retired,
        vec![
            "continuity.sqlite3".to_owned(),
            "sessions.sqlite3".to_owned()
        ],
        "{applied:#?}"
    );
    assert_eq!(native_layout_status(dir.path()), NativeLayoutStatus::Native);
    let backup = applied
        .backups
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("sessions.sqlite3."))
        })
        .unwrap_or_else(|| panic!("the sessions database's backup: {applied:#?}"));
    let schedules: i64 = rusqlite::Connection::open(backup)
        .expect("open the backup")
        .query_row("SELECT COUNT(*) FROM schedule_schedules", [], |row| {
            row.get(0)
        })
        .expect("count the backup's schedules");
    assert_eq!(schedules, 2);
}

/// A session database under a name no release reads (here a copy of the
/// projection, as a 0.9.3 store_path override would leave it) refuses the
/// census before anything is read, and names the file.
#[tokio::test]
async fn a_session_database_under_an_unknown_name_refuses_the_census() {
    let dir = unpack("s1b-plain");
    std::fs::copy(dir.path().join("sessions.sqlite3"), dir.path().join("x.db")).expect("copy");
    let report = census(dir.path()).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::UnrecognizedSessionDatabase { file }] if file == "x.db"
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");
    assert!(
        matches!(
            report.layout,
            Some(NativeLayoutStatus::NeedsMigration { ref reasons })
                if reasons.iter().any(|reason| matches!(
                    reason,
                    meerkat_meta::storage_migrate_native::NativeLayoutReason::UnrecognizedSessionDatabase { file }
                        if file == "x.db"
                ))
        ),
        "{report:#?}"
    );
}

/// A runtime whose WholeBlob authority references a missing body (no
/// release can load it) refuses the census until the operator names it with
/// --discard-bodyless; then --apply discards that authority after the
/// backup and the directory reads native. Naming a healthy session refuses.
/// The projection still holds a copy of the session, so once the authority
/// is gone that copy is the session's only one, and --apply imports it, as
/// a rerun after the discard would.
#[tokio::test]
async fn a_bodiless_runtime_refuses_until_the_operator_discards_it() {
    let dir = unpack("s1b-plain");
    edit(
        dir.path(),
        "runtime.sqlite",
        "DELETE FROM runtime_whole_blob_bodies WHERE blob_sha256 = (SELECT blob_sha256 FROM \
         runtime_whole_blob_authority ORDER BY runtime_id LIMIT 1)",
    );
    let report = census(dir.path()).await;
    let bodiless: Vec<&SessionCensusRow> = report
        .sessions
        .iter()
        .filter(|row| row.class == SessionCensusClass::BodilessRuntime)
        .collect();
    assert_eq!(bodiless.len(), 1, "{report:#?}");
    assert!(!bodiless[0].admitted, "{report:#?}");
    let session = bodiless[0].session_id.clone();
    let healthy = report
        .sessions
        .iter()
        .find(|row| row.class == SessionCensusClass::Equal)
        .expect("a healthy session")
        .session_id
        .clone();

    let named = migrate_state_dir_native(
        &NativeMigrateRequest::new(dir.path(), meerkat_meta::MigrateMode::DryRun)
            .with_discard_bodyless(healthy.clone()),
    )
    .await;
    assert!(
        named.refusals.iter().any(|refusal| matches!(
            refusal,
            NativeMigrateRefusal::DiscardNotBodiless { session_id } if *session_id == healthy
        )),
        "{named:#?}"
    );

    let applied = migrate_state_dir_native(
        &NativeMigrateRequest::new(dir.path(), meerkat_meta::MigrateMode::Apply)
            .with_discard_bodyless(session.clone()),
    )
    .await;
    assert!(applied.refusals.is_empty(), "{applied:#?}");
    assert_eq!(
        row_of(&applied, &session).class,
        SessionCensusClass::RuntimeMissing
    );
    assert_eq!(
        applied.applied.as_ref().map(|outcome| outcome
            .imported
            .iter()
            .map(|import| import.session_id.clone())
            .collect::<Vec<_>>()),
        Some(vec![session.clone()]),
        "{applied:#?}"
    );
    assert_eq!(
        applied.applied.as_ref().map(|outcome| outcome
            .discarded
            .iter()
            .map(|discarded| discarded.session_id.clone())
            .collect::<Vec<_>>()),
        Some(vec![session.clone()]),
        "{applied:#?}"
    );
    // The other sessions came through untouched.
    assert!(
        applied
            .sessions
            .iter()
            .filter(|row| row.session_id != session)
            .all(|row| row.class == SessionCensusClass::Equal),
        "{applied:#?}"
    );
    assert_eq!(
        meerkat_meta::storage_migrate_native::native_layout_status(dir.path()),
        NativeLayoutStatus::Native
    );
}

/// A body-less WholeBlob authority under the session's legacy alias (the
/// bare session id) is a shape no released Core writes, and Core reads and
/// clears authority only under the session's own runtime id, so
/// `--discard-bodyless` could not discard it. The census refuses the session
/// as unreadable, and `--apply` refuses before it writes anything: no
/// journal, no backup, `runtime.sqlite` unchanged. (A body-less authority
/// under the session's runtime id is discarded:
/// `a_bodiless_runtime_refuses_until_the_operator_discards_it`.)
#[tokio::test]
async fn a_bodiless_authority_under_the_legacy_alias_is_refused_before_apply_writes() {
    let dir = unpack("s1b-plain");
    edit(
        dir.path(),
        "runtime.sqlite",
        "DELETE FROM runtime_whole_blob_bodies WHERE blob_sha256 = (SELECT blob_sha256 FROM \
         runtime_whole_blob_authority ORDER BY runtime_id LIMIT 1)",
    );
    let session = census(dir.path())
        .await
        .sessions
        .iter()
        .find(|row| row.class == SessionCensusClass::BodilessRuntime)
        .expect("a body-less session")
        .session_id
        .clone();
    let session_id = meerkat_core::types::SessionId::parse(&session).expect("a session id");
    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(&session_id).0;
    let alias = meerkat_runtime::LogicalRuntimeId::legacy_session_uuid_alias(&session_id).0;
    // Give the session's legacy alias the same body-less authority.
    edit(
        dir.path(),
        "runtime.sqlite",
        &format!(
            "INSERT INTO runtime_whole_blob_authority (runtime_id, authority_version, \
             session_id, store_revision, blob_sha256) SELECT '{alias}', authority_version, \
             session_id, store_revision, blob_sha256 FROM runtime_whole_blob_authority WHERE \
             runtime_id = '{runtime_id}'"
        ),
    );

    let report = census(dir.path()).await;
    let row = row_of(&report, &session);
    assert_eq!(row.class, SessionCensusClass::Unreadable, "{report:#?}");
    assert!(!row.admitted, "{row:#?}");
    assert!(row.detail.contains(&alias), "{row:#?}");
    assert!(
        row.detail.contains("--discard-bodyless cannot discard it"),
        "{row:#?}"
    );

    let before = snapshot(dir.path());
    let applied = migrate_state_dir_native(
        &NativeMigrateRequest::new(dir.path(), meerkat_meta::MigrateMode::Apply)
            .with_discard_bodyless(session.clone()),
    )
    .await;
    assert!(
        matches!(
            applied.refusals.as_slice(),
            [NativeMigrateRefusal::SessionsRefused { count: 1 }]
        ),
        "{applied:#?}"
    );
    assert!(applied.applied.is_none(), "{applied:#?}");
    assert_eq!(
        snapshot(dir.path()),
        before,
        "a refused --apply writes nothing: no journal, no backup"
    );
}

/// Two spellings of one database side by side refuse before anything is
/// read, as the 0.9.4 layout refuses them; storage-migrate resolves twins.
#[tokio::test]
async fn twin_spellings_of_one_database_are_refused_before_anything_is_read() {
    let dir = unpack("s1b-plain");
    std::fs::write(dir.path().join("sessions.db"), b"").expect("touch");
    let report = census(dir.path()).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::SpellingTwins { slot, files }]
                if slot == "sessions.sqlite3" && *files == ["sessions.sqlite3", "sessions.db"]
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");
}

/// The tripwire storage-migrate --session-db leaves at a legacy spelling
/// (the session store's retired stamp) beside the canonical file is neither
/// a twin nor a projection: the census reads the canonical file.
#[tokio::test]
async fn a_retired_stamp_at_a_legacy_spelling_is_not_a_twin() {
    let dir = unpack("s1b-plain");
    {
        let conn = rusqlite::Connection::open(dir.path().join("sessions.db")).expect("stamp");
        conn.execute_batch(&format!(
            "CREATE TABLE meerkat_schema (domain TEXT PRIMARY KEY, version INTEGER NOT NULL); \
             INSERT INTO meerkat_schema (domain, version) VALUES ('{}', {});",
            meerkat_store::sqlite_store::SESSION_STORE_DOMAIN.name,
            meerkat_meta::storage_migrate_native::RETIRED_PROJECTION_SCHEMA_VERSION
        ))
        .expect("write the stamp");
    }
    let report = census(dir.path()).await;
    assert_eq!(
        report.shape,
        Some(GatewayStateShape::SessionsProjection),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
}

/// Rename a database and its log to another spelling of its slot.
fn respell(dir: &Path, from: &str, to: &str) {
    for suffix in ["", "-wal", "-shm"] {
        let source = dir.join(format!("{from}{suffix}"));
        if source.exists() {
            std::fs::rename(&source, dir.join(format!("{to}{suffix}"))).expect("respell");
        }
    }
}

/// A lone legacy spelling is the projection, read and retired in place
/// under its own name, exactly the file the 0.9.4 layout resolves: no
/// rename first. A 0.9.2 continuity.db reads like continuity.sqlite3.
#[tokio::test]
async fn a_lone_legacy_spelling_is_migrated_in_place() {
    let dir = unpack("s1b-plain");
    respell(dir.path(), "sessions.sqlite3", "sessions.db");
    let report = census(dir.path()).await;
    assert_eq!(
        report.shape,
        Some(GatewayStateShape::SessionsProjection),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
    let applied = migrate_state_dir_native(&NativeMigrateRequest::new(
        dir.path(),
        meerkat_meta::MigrateMode::Apply,
    ))
    .await;
    assert!(applied.refusals.is_empty(), "{applied:#?}");
    assert_eq!(
        applied
            .applied
            .as_ref()
            .map(|outcome| outcome.retired.clone()),
        Some(vec!["sessions.db".to_owned()]),
        "{applied:#?}"
    );
    assert!(!dir.path().join("sessions.sqlite3").exists());
    assert_eq!(
        meerkat_meta::storage_migrate_native::native_layout_status(dir.path()),
        NativeLayoutStatus::Native
    );

    let dir = unpack("s1-roster");
    respell(dir.path(), "continuity.sqlite3", "continuity.db");
    let report = census(dir.path()).await;
    assert_eq!(
        report.shape,
        Some(GatewayStateShape::ContinuityProjection),
        "{report:#?}"
    );
    assert!(!report.sessions.is_empty(), "{report:#?}");
    assert!(
        report.sessions.iter().all(|row| row.admitted),
        "{report:#?}"
    );
}

#[tokio::test]
async fn no_runtime_store_or_no_directory_is_refused() {
    let dir = unpack("s1b-plain");
    std::fs::remove_file(dir.path().join("runtime.sqlite")).expect("remove");
    let report = census(dir.path()).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::NoRuntimeStore]
        ),
        "{report:#?}"
    );
    let missing = dir.path().join("absent");
    let report = census(&missing).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::NotADirectory]
        ),
        "{report:#?}"
    );
}

/// A directory already in the native layout has nothing to migrate, in either
/// mode: the census runs only where the startup predicate finds 0.9.3 storage.
#[tokio::test]
async fn a_native_directory_has_nothing_to_migrate() {
    let dir = tempfile::tempdir().expect("state dir");
    let runtime = dir.path().join("runtime.sqlite");
    meerkat_store::SqliteSessionStore::open(&runtime).expect("session store");
    drop(meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&runtime).expect("runtime store"));
    for mode in [MigrateMode::DryRun, MigrateMode::Apply] {
        let report = migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), mode)).await;
        assert_eq!(
            report.layout,
            Some(NativeLayoutStatus::Native),
            "{report:#?}"
        );
        assert!(report.refusals.is_empty(), "{report:#?}");
        assert!(
            report.sessions.is_empty() && report.shape.is_none(),
            "{report:#?}"
        );
    }
}

/// An `--apply` that did not finish leaves its journal; the census refuses
/// until the apply is finished.
#[tokio::test]
async fn an_unfinished_apply_refuses_the_census() {
    let dir = unpack("s1b-plain");
    std::fs::write(
        dir.path().join("storage-migrate-native.journal.json"),
        b"{}",
    )
    .expect("journal");
    let report = census(dir.path()).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::MigrationIncomplete { journal }]
                if journal.ends_with("storage-migrate-native.journal.json")
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");
}

/// The continuity session store does not list its sessions, so the census
/// reads their ids from the tables that hold the bodies: a session no record
/// binds (an identity reset away from it, say) is still compared. Deleting
/// the record leaves k1-beta's member live with no record, which the
/// identity census refuses (U3); the session rows are unaffected.
#[tokio::test]
async fn a_session_no_record_binds_is_still_compared() {
    let dir = unpack("s1-roster");
    let beta = session_of(dir.path(), "agent:k1-beta");
    edit(
        dir.path(),
        "continuity.sqlite3",
        "DELETE FROM continuity_records WHERE identity = 'agent:k1-beta'",
    );
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 3)]),
        "{report:#?}"
    );
    assert!(row_of(&report, &beta).identities.is_empty(), "{report:#?}");
    assert_census_refusals_disagreeing(&report, 0, &["agent:k1-beta"]);
}

/// A session only the projection holds and no record binds is a re-mint
/// seed like any other: its row plans the import. Deleting the record
/// leaves k1-gamma's member live with no record, which the identity census
/// refuses (U3), so the census refuses the run and nothing is imported in
/// it; the session row is unaffected.
#[tokio::test]
async fn a_projection_only_session_no_record_binds_is_imported() {
    let dir = unpack("s1-runtime-missing");
    let gamma = session_of(dir.path(), "agent:k1-gamma");
    edit(
        dir.path(),
        "continuity.sqlite3",
        "DELETE FROM continuity_records WHERE identity = 'agent:k1-gamma'",
    );
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2), ("RuntimeMissing", 1)]),
        "{report:#?}"
    );
    let row = row_of(&report, &gamma);
    assert_eq!(row.class, SessionCensusClass::RuntimeMissing, "{row:#?}");
    assert!(row.identities.is_empty(), "{row:#?}");
    assert_imported_row(&report);
    assert_census_refusals_disagreeing(&report, 0, &["agent:k1-gamma"]);
}

/// A record that binds a session the projection holds no body for is no
/// copy: Core's copy in runtime.sqlite is everything 0.9.3 serves.
#[tokio::test]
async fn a_bound_session_without_a_projection_body_keeps_cores_copy() {
    let dir = unpack("s1-roster");
    let beta = session_of(dir.path(), "agent:k1-beta");
    edit(
        dir.path(),
        "continuity.sqlite3",
        &format!(
            "DELETE FROM session_snapshots WHERE session_id = '{beta}';
             DELETE FROM continuity_session_heads WHERE session_id = '{beta}';
             DELETE FROM continuity_strand_messages WHERE session_id = '{beta}';
             DELETE FROM continuity_session_rewrites WHERE session_id = '{beta}';"
        ),
    );
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2), ("ProjectionMissing", 1)]),
        "{report:#?}"
    );
    let row = row_of(&report, &beta);
    assert_eq!(row.class, SessionCensusClass::ProjectionMissing, "{row:#?}");
    assert!(row.admitted, "{row:#?}");
    assert_eq!(row.identities, vec!["agent:k1-beta".to_owned()], "{row:#?}");
    assert_only_census_refusals(&report, 0);
}

/// A projection body that does not decode cannot seed a re-mint: its row is
/// Unreadable and names the copy that failed.
#[tokio::test]
async fn an_undecodable_projection_body_is_unreadable() {
    let dir = unpack("s1-runtime-missing");
    let gamma = session_of(dir.path(), "agent:k1-gamma");
    edit(
        dir.path(),
        "continuity.sqlite3",
        &format!(
            "UPDATE continuity_session_heads SET head_json = X'7b' WHERE session_id = '{gamma}';
             UPDATE session_snapshots SET data = X'7b' WHERE session_id = '{gamma}';"
        ),
    );
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2), ("Unreadable", 1)]),
        "{report:#?}"
    );
    let row = row_of(&report, &gamma);
    assert_eq!(row.class, SessionCensusClass::Unreadable, "{row:#?}");
    assert!(row.detail.contains("projection copy"), "{row:#?}");
    assert_only_census_refusals(&report, 1);
}

/// A committed snapshot that does not load refuses its own row; the other
/// sessions are still classified.
#[tokio::test]
async fn a_committed_snapshot_that_does_not_load_is_its_own_row() {
    let dir = unpack("s1-roster");
    let beta = session_of(dir.path(), "agent:k1-beta");
    edit(
        dir.path(),
        "runtime.sqlite",
        &format!(
            "UPDATE runtime_whole_blob_bodies SET session_snapshot = X'7b' WHERE blob_sha256 = \
             (SELECT blob_sha256 FROM runtime_whole_blob_authority WHERE session_id = '{beta}')"
        ),
    );
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 2), ("Unreadable", 1)]),
        "{report:#?}"
    );
    let row = row_of(&report, &beta);
    assert_eq!(row.class, SessionCensusClass::Unreadable, "{row:#?}");
    assert!(row.detail.contains("committed session"), "{row:#?}");
    assert_only_census_refusals(&report, 1);
}

/// A runtime that holds the directory refuses the census: a 0.9.3 gateway
/// holds Core's runtime custody lock while its store is open, and a 0.9.4
/// runtime holds `meta-runtime.lock`. Unheld lock files change nothing.
#[tokio::test]
async fn a_runtime_using_the_directory_refuses_the_census() {
    let dir = unpack("s1b-plain");
    let lock_file = |name: &str| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.path().join(name))
            .expect("lock file")
    };

    // Core's ordinary runtime owner holds its lifetime lock shared.
    let custody = lock_file("runtime.sqlite.execution-custody");
    custody.try_lock_shared().expect("custody lock");
    let report = census(dir.path()).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::RuntimeInUse { file }]
                if file.ends_with("runtime.sqlite.execution-custody")
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");
    drop(custody);

    let meta_runtime = lock_file("meta-runtime.lock");
    meta_runtime.try_lock().expect("meta runtime lock");
    let report = census(dir.path()).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::RuntimeInUse { file }] if file.ends_with("meta-runtime.lock")
        ),
        "{report:#?}"
    );
    drop(meta_runtime);

    let before = snapshot(dir.path());
    let report = census(dir.path()).await;
    assert_eq!(
        class_counts(&report),
        counts(&[("Equal", 3)]),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
    assert_eq!(snapshot(dir.path()), before, "the census changed a file");
}

/// The scratch copies go beside the state directory unless the request names
/// another place, and are gone when the run ends.
#[tokio::test]
async fn scratch_copies_go_where_the_request_says_and_are_removed() {
    let outer = tempfile::tempdir().expect("outer dir");
    let state = outer.path().join("state");
    std::fs::create_dir(&state).expect("state dir");
    unpack_into("s1b-plain", &state);
    let report = census(&state).await;
    assert_only_census_refusals(&report, 0);
    let beside: Vec<_> = std::fs::read_dir(outer.path())
        .expect("outer dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(beside, vec![std::ffi::OsString::from("state")]);

    let scratch = tempfile::tempdir().expect("scratch dir");
    let report = migrate_state_dir_native(
        &NativeMigrateRequest::new(&state, MigrateMode::DryRun).with_scratch_dir(scratch.path()),
    )
    .await;
    assert_only_census_refusals(&report, 0);
    assert_eq!(
        std::fs::read_dir(scratch.path())
            .expect("scratch dir")
            .count(),
        0,
        "scratch copies left behind"
    );

    // The named place is the one used: one that cannot hold the copies
    // refuses typed and names it.
    let missing = scratch.path().join("absent");
    let report = migrate_state_dir_native(
        &NativeMigrateRequest::new(&state, MigrateMode::DryRun).with_scratch_dir(&missing),
    )
    .await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::ScratchCopyFailed { detail, .. }] if detail.contains("absent")
        ),
        "{report:#?}"
    );
}

/// Without --scratch-dir the copies go beside the state directory, in its
/// parent: a parent that cannot hold them refuses typed and names it.
#[cfg(unix)]
#[tokio::test]
async fn the_default_scratch_place_is_the_state_directorys_parent() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |bits| std::fs::Permissions::from_mode(bits);
    let outer = tempfile::tempdir().expect("outer dir");
    let state = outer.path().join("state");
    std::fs::create_dir(&state).expect("state dir");
    unpack_into("s1b-plain", &state);
    std::fs::set_permissions(outer.path(), mode(0o555)).expect("make the parent read-only");
    // A process that writes through the mode (root) cannot show where the
    // copies go; the case then proves nothing and is skipped.
    let probe = outer.path().join("probe");
    if std::fs::write(&probe, b"").is_ok() {
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(outer.path(), mode(0o755)).expect("restore the parent");
        eprintln!("skipped: this process writes into a read-only directory");
        return;
    }
    let report = census(&state).await;
    std::fs::set_permissions(outer.path(), mode(0o755)).expect("restore the parent");
    let parent = outer.path().display().to_string();
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::ScratchCopyFailed { detail, .. }] if detail.contains(&parent)
        ),
        "{report:#?}"
    );
}

/// The census on every released 0.9.3 H-local capture, row by row.
///
/// The expected class of each committed session follows the capture's plan:
/// - a session compacted past the retention (the MANIFEST's `reanchored` rows)
///   is admitted, because Core lays out the retained graph from its retired
///   base: Equal when its continuity record binds it, else ProjectionMissing
///   (its projection copy is gone);
/// - a session its identity had before a delete_identity (`agent:hl-07` in the
///   H-local plan, the `rematerialized` rows' previous sessions) lost its
///   continuity copy, so it is ProjectionMissing;
/// - in `killed`, `agent:hl-14`'s projection lags its committed compaction
///   (ProjectionBehind);
/// - everything else is Equal.
///
/// Each row's identities must be exactly the identities whose continuity
/// record binds the session, read straight from the staged
/// continuity.sqlite3. Pinning class and identity per row means a swapped
/// attribution or a class that is merely admitted cannot pass.
#[tokio::test]
async fn every_released_h_local_capture_reads_as_its_plan() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    for variant in Variant::ALL {
        let temp = tempfile::tempdir().expect("temp dir");
        let state = stage_variant(variant, temp.path());
        let manifest = variant.manifest();
        let report = census(&state).await;
        let name = variant.dir_name();

        let owner_of = |session: &str| -> String {
            let mut owners: Vec<&str> = manifest["identities"]
                .as_object()
                .expect("identities")
                .iter()
                .filter(|(_, info)| {
                    info["sessions_seen"]
                        .as_array()
                        .is_some_and(|seen| seen.iter().any(|s| s == session))
                })
                .map(|(identity, _)| identity.as_str())
                .collect();
            // A member the host made itself owns its session too.
            if manifest["direct_member"]["sessions_seen"]
                .as_array()
                .is_some_and(|seen| seen.iter().any(|s| s == session))
            {
                owners.push(
                    manifest["direct_member"]["member_id"]
                        .as_str()
                        .expect("member id"),
                );
            }
            assert_eq!(owners.len(), 1, "{name}: {session} has owners {owners:?}");
            owners[0].to_owned()
        };
        let reanchored: BTreeMap<String, bool> = manifest["reanchored"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|row| {
                        (
                            row["session_id"].as_str().expect("session").to_owned(),
                            row["bound"].as_bool() == Some(true),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut runtime_only: Vec<String> = manifest["identities"]["agent:hl-07"]["sessions_seen"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|seen| {
                **seen != manifest["identities"]["agent:hl-07"]["final_status"]["session_id"]
            })
            .map(|seen| seen.as_str().expect("session").to_owned())
            .collect();
        runtime_only.extend(
            manifest["rematerialized"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|row| row["previous_runtime_only"].as_bool() == Some(true))
                .map(|row| {
                    row["previous_session_id"]
                        .as_str()
                        .expect("session")
                        .to_owned()
                }),
        );

        runtime_only.extend(
            manifest["direct_member"]["sessions_seen"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|seen| seen.as_str().expect("session").to_owned()),
        );

        let bindings: BTreeMap<String, Vec<String>> = {
            let connection = rusqlite::Connection::open_with_flags(
                state.join("continuity.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("open the staged continuity copy read-only");
            let mut statement = connection
                .prepare("SELECT identity, session_id FROM continuity_records")
                .expect("prepare");
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .expect("query");
            let mut bindings: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for row in rows {
                let (identity, session) = row.expect("row");
                bindings.entry(session).or_default().push(identity);
            }
            bindings
        };

        let committed: Vec<&String> = manifest["committed_sessions"]
            .as_object()
            .expect("committed sessions")
            .keys()
            .collect();
        assert_eq!(
            report.sessions.len(),
            committed.len(),
            "{name}: one row per committed session\n{report:#?}"
        );
        for session in committed {
            let row = row_of(&report, session);
            let owner = owner_of(session);
            let expected = if let Some(bound) = reanchored.get(session.as_str()) {
                if *bound {
                    SessionCensusClass::Equal
                } else {
                    SessionCensusClass::ProjectionMissing
                }
            } else if runtime_only.contains(session) {
                SessionCensusClass::ProjectionMissing
            } else if variant == Variant::Killed && owner == "agent:hl-14" {
                SessionCensusClass::ProjectionBehind
            } else {
                SessionCensusClass::Equal
            };
            assert_eq!(row.class, expected, "{name}: {session} ({owner})\n{row:#?}");
            let mut bound = bindings.get(session.as_str()).cloned().unwrap_or_default();
            bound.sort();
            let mut listed = row.identities.clone();
            listed.sort();
            assert_eq!(listed, bound, "{name}: {session} ({owner})\n{row:#?}");
        }

        // The re-anchored bodies pass Core's retained layout: the bound one
        // relates Equal to its projection, and the runtime-only one has no
        // projection copy.
        for row in manifest["reanchored"].as_array().into_iter().flatten() {
            let session = row["session_id"].as_str().expect("session");
            let row_of_session = row_of(&report, session);
            let relation = if row["bound"].as_bool() == Some(true) {
                "the projection head is at the committed head"
            } else {
                "the projection holds no copy"
            };
            assert!(
                row_of_session.detail.starts_with(relation),
                "{name}: {session}: {row_of_session:#?}"
            );
            assert!(
                row_of_session.admitted,
                "{name}: {session}: {row_of_session:#?}"
            );
        }
        // Every identity-first spawn of the released 0.9.3 labels its member
        // agent_identity with the identity: a first spawn, a wake's, and a
        // reset successor's (the H-local plans reset hl-06 once and hl-09
        // twice). A member the host made itself carries only its own labels.
        let history =
            meerkat_mob::MobStorage::read_member_binding_history(&state.join("event_log.sqlite3"))
                .await
                .expect("read the capture's member history");
        for (identity, facts) in manifest["identities"].as_object().expect("identities") {
            if facts["sessions_seen"].as_array().is_none_or(Vec::is_empty) {
                continue;
            }
            let member = meerkat_meta::member_comms_id::roster_member_id_for_identity(identity);
            let binding = history
                .last_binding(&member)
                .unwrap_or_else(|| panic!("{name}: Core holds no binding of {identity}"));
            assert_eq!(
                binding.labels().get("agent_identity").map(String::as_str),
                Some(identity.as_str()),
                "{name}: {identity} at generation {:?}",
                binding.generation()
            );
        }
        if let Some(member) = manifest["direct_member"]["member_id"].as_str() {
            let binding = history
                .last_binding(&meerkat_mob::AgentIdentity::from(member))
                .unwrap_or_else(|| panic!("{name}: Core holds no binding of {member}"));
            assert!(
                !binding.labels().contains_key("agent_identity"),
                "{name}: {member}: {binding:?}"
            );
        }
        assert_only_census_refusals(&report, 0);
    }
}

/// The census of the released direct-member capture: `hl-direct`, the member
/// the host made itself with the stdio gateway's `meta/ensure_member` (no
/// continuity record), and `agent:hl-01`, an identity the roster lists.
async fn direct_member_census(state: &Path) -> (NativeMigrateReport, String, String) {
    let manifest = released_0_9_3_h_local::Variant::DirectMember.manifest();
    let member = manifest["direct_member"]["member_id"]
        .as_str()
        .expect("member id")
        .to_owned();
    let session = manifest["direct_member"]["session_id"]
        .as_str()
        .expect("session")
        .to_owned();
    (census(state).await, member, session)
}

fn identity_row<'a>(report: &'a NativeMigrateReport, identity: &str) -> &'a IdentityCensusRow {
    report
        .identities
        .iter()
        .find(|row| row.identity == identity)
        .unwrap_or_else(|| panic!("no identity row for {identity}: {report:#?}"))
}

/// A member the host made directly on the mob, which 0.9.3 never recorded,
/// migrates as a raw member: the census admits it with no first-boot row (its
/// session is held by runtime.sqlite alone, admitted too), and `--apply`
/// leaves Core's member and its transcript as 0.9.3 left them.
#[tokio::test]
async fn a_member_the_host_made_directly_on_the_mob_migrates_as_a_raw_member() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::DirectMember, temp.path());
    let manifest = Variant::DirectMember.manifest();
    let (report, member, session) = direct_member_census(&state).await;

    let raw = identity_row(&report, &member);
    assert_eq!(raw.class, IdentityCensusClass::RawMember, "{report:#?}");
    assert!(raw.admitted && raw.action.is_none(), "{raw:#?}");
    assert_eq!(
        identity_row(&report, "agent:hl-01").class,
        IdentityCensusClass::Bound
    );
    let held = row_of(&report, &session);
    assert_eq!(
        held.class,
        SessionCensusClass::ProjectionMissing,
        "{held:#?}"
    );
    assert!(held.admitted && held.identities.is_empty(), "{held:#?}");
    assert!(
        report.first_boot.iter().all(|row| match row {
            FirstBootRowPlan::Resume { identity, .. }
            | FirstBootRowPlan::Fresh { identity, .. } => identity != &member,
            _ => true,
        }),
        "a raw member gets no first-boot row: {:#?}",
        report.first_boot
    );
    assert_only_census_refusals(&report, 0);

    let mob_store = state.join("event_log.sqlite3");
    let member_id = meerkat_mob::AgentIdentity::from(member.as_str());
    let before = meerkat_mob::MobStorage::read_member_binding_history(&mob_store)
        .await
        .expect("read the 0.9.3 member history")
        .last_binding(&member_id)
        .cloned()
        .expect("the raw member's binding");
    assert!(before.is_live(), "{before:?}");
    let labels: BTreeMap<String, String> =
        serde_json::from_value(manifest["direct_member"]["labels"].clone())
            .expect("the raw member's labels");
    assert!(!labels.is_empty() && !labels.contains_key("agent_identity"));
    assert_eq!(
        before.labels(),
        &labels,
        "the raw member carries the labels 0.9.3 spawned it with"
    );
    assert_eq!(
        before.session_id().map(ToString::to_string),
        Some(session.clone())
    );

    let applied =
        migrate_state_dir_native(&NativeMigrateRequest::new(&state, MigrateMode::Apply)).await;
    assert!(applied.refusals.is_empty(), "{applied:#?}");
    assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);

    let after = meerkat_mob::MobStorage::read_member_binding_history(&mob_store)
        .await
        .expect("read the member history after the apply");
    assert_eq!(
        after.last_binding(&member_id),
        Some(&before),
        "Core keeps the raw member as 0.9.3 left it"
    );
    assert_eq!(
        after
            .last_binding(&member_id)
            .map(meerkat_mob::MemberBinding::labels),
        Some(&labels),
        "the raw member's labels survive --apply"
    );
    let native = meerkat_store::SqliteSessionStore::open(state.join("runtime.sqlite"))
        .expect("native session store");
    let kept = native
        .load(&meerkat_core::types::SessionId::parse(&session).expect("session id"))
        .await
        .expect("load")
        .expect("the raw member's session");
    assert_eq!(
        u64::try_from(kept.messages().len()).expect("message count"),
        manifest["committed_sessions"][session.as_str()]["messages"]
            .as_u64()
            .expect("message count"),
        "the converted session holds every message 0.9.3 committed"
    );
    let transcript = serde_json::to_string(kept.messages()).expect("transcript json");
    for turn in [
        "boot 1 hello from the host to hl-direct",
        "boot 2 follow-up from the host to hl-direct",
    ] {
        assert!(transcript.contains(turn), "{turn}: {transcript}");
    }
}

/// The raw member rule does not admit a member the identity runtime spawned:
/// with `agent:hl-01`'s continuity record gone, its live member (whose spawn
/// carries the identity-first label) is still refused (U3), while the raw
/// member stays admitted.
#[tokio::test]
async fn an_identity_member_without_its_record_is_still_refused() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::DirectMember, temp.path());
    edit(
        &state,
        "continuity.sqlite3",
        "DELETE FROM continuity_records WHERE identity = 'agent:hl-01';",
    );
    let (report, member, _) = direct_member_census(&state).await;
    assert_census_refusals_disagreeing(&report, 0, &["agent:hl-01"]);
    let refused = identity_row(&report, "agent:hl-01");
    assert!(
        refused.detail.contains("has a live member for it"),
        "{refused:#?}"
    );
    assert_eq!(
        identity_row(&report, &member).class,
        IdentityCensusClass::RawMember
    );
}

/// A record that binds the raw member's session to another identity is
/// still refused (R4): the raw member rule reads only members no claim names.
#[tokio::test]
async fn a_record_on_the_raw_members_session_is_still_refused() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::DirectMember, temp.path());
    let manifest = Variant::DirectMember.manifest();
    let session = manifest["direct_member"]["session_id"]
        .as_str()
        .expect("session");
    edit(
        &state,
        "continuity.sqlite3",
        &format!(
            "UPDATE continuity_records SET session_id = '{session}' \
             WHERE identity = 'agent:hl-01';"
        ),
    );
    let (report, member, _) = direct_member_census(&state).await;
    assert_census_refusals_disagreeing(&report, 0, &["agent:hl-01"]);
    let refused = identity_row(&report, "agent:hl-01");
    assert!(
        refused
            .detail
            .contains(&format!("last bound the session to identity {member}")),
        "{refused:#?}"
    );
    assert_eq!(
        identity_row(&report, &member).class,
        IdentityCensusClass::RawMember
    );
}

/// The raw member rule reads the identity-first label, which 0.9.3 writes on
/// every spawn of an identity's member, a reset successor's included:
/// agent:hl-09, reset twice in the released bootable capture, is live at
/// generation 2, and with its record gone it is still refused (U3).
#[tokio::test]
async fn a_reset_successor_without_its_record_is_still_refused() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Bootable, temp.path());
    let manifest = Variant::Bootable.manifest();
    assert_eq!(
        manifest["identities"]["agent:hl-09"]["final_status"]["generation"], 2,
        "hl-09 lives at its second reset successor"
    );
    edit(
        &state,
        "continuity.sqlite3",
        "DELETE FROM continuity_records WHERE identity = 'agent:hl-09';",
    );
    let report = census(&state).await;
    assert_census_refusals_disagreeing(&report, 0, &["agent:hl-09"]);
    let refused = identity_row(&report, "agent:hl-09");
    assert!(
        refused.detail.contains("has a live member for it"),
        "{refused:#?}"
    );
}

/// `agent:hl-14`'s session in the released killed/ capture: its projection
/// copy is a prefix of the parent of the rewrite the runtime committed when
/// the gateway was killed mid-compaction.
const KILLED_HL14_SESSION: &str = "01a153bc-e035-79c0-bc2f-510b8fc8ba21";

/// The remedy of a suffix row on a session `agent:hl-14` binds and 0.9.3 left
/// live: the steps a published 0.9.3 binary was seen to converge.
const HL14_RESTART_REMEDY: &str = "keep agent:hl-14 on the roster, start the directory once \
                                   with 0.9.3 and stop it cleanly; 0.9.3 re-seeds the session \
                                   (verified with the published binary for an idle session); \
                                   then rerun the census";

/// Mint the inverse-append suffix shape on `agent:hl-14`'s session of a
/// staged killed/ capture, through the continuity store's own write path and
/// a writer registered from the session's continuity record: the projection
/// copy grows to the committed rewrite's whole parent plus one row the
/// runtime never committed. No published 0.9.3 run produces the shape on
/// demand (it comes from a retire torn by its own compaction), so the test
/// writes it. The parent is read from a copy of runtime.sqlite, so the
/// staged runtime store keeps the released bytes.
async fn mint_hl14_suffix(state: &Path) {
    let id = meerkat_core::types::SessionId::parse(KILLED_HL14_SESSION).expect("session id");
    let scratch = tempfile::tempdir().expect("runtime copy dir");
    for file in ["runtime.sqlite", "runtime.sqlite-wal", "runtime.sqlite-shm"] {
        if state.join(file).exists() {
            std::fs::copy(state.join(file), scratch.path().join(file)).expect("copy runtime store");
        }
    }
    let parent: Vec<meerkat_core::Message> = {
        let store = meerkat_runtime::SqliteRuntimeStore::new_whole_blob(
            scratch.path().join("runtime.sqlite"),
        )
        .expect("open the runtime copy");
        let committed = store
            .load_committed_whole_blob_snapshot(&meerkat_runtime::LogicalRuntimeId::for_session(
                &id,
            ))
            .await
            .expect("load the committed session")
            .expect("hl-14's committed session")
            .session_arc();
        let sealed = committed
            .validated_transcript_history_state()
            .expect("rewrite history")
            .expect("hl-14's session carries a rewrite");
        let commit = sealed.state().commits().next_back().expect("one rewrite");
        committed
            .with_validated_transcript_rewrite_parent_projection(&sealed, commit)
            .expect("the rewrite's parent")
            .messages()
            .to_vec()
    };
    let continuity: Arc<dyn ContinuityStore> = Arc::new(
        LocalContinuityStore::open(state.join("continuity.sqlite3")).expect("open continuity copy"),
    );
    let adapter = ContinuitySessionStoreAdapter::new(Arc::clone(&continuity));
    let (record, fencing_token, fence_current) = continuity
        .resolve_record_by_session(&id)
        .await
        .expect("resolve the record")
        .expect("hl-14's record binds the session");
    adapter
        .register_session(
            &id,
            SessionRuntimeState {
                identity: record.identity.clone(),
                generation: record.generation,
                fencing_token,
                checkpoint_version: fence_current,
            },
        )
        .await
        .expect("register the writer");
    let mut projection = adapter
        .load(&id)
        .await
        .expect("load")
        .expect("projection copy");
    let have = projection.messages().len();
    assert!(
        have < parent.len(),
        "the projection copy is behind the parent"
    );
    assert_eq!(
        meerkat_core::transcript_messages_digest(projection.messages()).expect("digest"),
        meerkat_core::transcript_messages_digest(&parent[..have]).expect("digest"),
        "the projection copy is a prefix of the rewrite's parent"
    );
    for message in &parent[have..] {
        projection.push(message.clone());
    }
    projection.push(meerkat_core::Message::User(
        meerkat_core::types::UserMessage::text("a row the runtime never committed"),
    ));
    adapter.save(&projection).await.expect("append the suffix");
}

/// The shape is minted on hl-14's session and the record that binds it is
/// then deleted. No 0.9.3 process can write a head-canonical session no
/// record binds (the facade needs the owner's registration), so 0.9.3 never
/// completes the repair that would keep the extra row: the census admits the
/// row, keeps Core's copy and names the dropped rows. The deletion leaves
/// hl-14's member live with no record, which the identity census refuses
/// (U3).
#[tokio::test]
async fn an_unbound_projection_suffix_is_admitted_and_drops_its_rows() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Killed, temp.path());
    mint_hl14_suffix(&state).await;
    edit(
        &state,
        "continuity.sqlite3",
        "DELETE FROM continuity_records WHERE identity = 'agent:hl-14'",
    );
    let report = census(&state).await;
    let row = row_of(&report, KILLED_HL14_SESSION);
    assert_eq!(
        row.class,
        SessionCensusClass::ProjectionSuffixUnbound,
        "{row:#?}"
    );
    assert!(row.admitted, "{row:#?}");
    assert!(row.identities.is_empty(), "{row:#?}");
    assert!(
        row.detail
            .contains("1 projection rows past the rewrite's parent"),
        "{row:#?}"
    );
    assert!(
        row.detail
            .ends_with("no continuity record binds the session"),
        "{row:#?}"
    );
    let effect = row.apply_effect.as_deref().expect("an apply effect");
    assert!(effect.contains("are dropped"), "{effect}");
    assert!(
        effect.ends_with("(the pre-apply backup of continuity.sqlite3 keeps them)"),
        "{effect}"
    );
    assert_eq!(row.action, None, "{row:#?}");
    assert_eq!(
        report.class_counts().get("projection_suffix_unbound"),
        Some(&1),
        "{report:#?}"
    );
    assert_census_refusals_disagreeing(&report, 0, &["agent:hl-14"]);
}

/// Control: the same shape with hl-14's record in place stays refused (a
/// 0.9.3 start keeps the row), and, hl-14's session being live, the remedy
/// names the 0.9.3 restart that converges it.
#[tokio::test]
async fn a_bound_projection_suffix_stays_refused_and_names_the_restart() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Killed, temp.path());
    mint_hl14_suffix(&state).await;
    let report = census(&state).await;
    let row = row_of(&report, KILLED_HL14_SESSION);
    assert_eq!(row.class, SessionCensusClass::ProjectionSuffix, "{row:#?}");
    assert!(!row.admitted, "{row:#?}");
    assert_eq!(row.identities, vec!["agent:hl-14".to_owned()], "{row:#?}");
    let action = row
        .action
        .as_deref()
        .expect("a refused row names an action");
    assert!(action.ends_with(HL14_RESTART_REMEDY), "{action}");
    assert_only_census_refusals(&report, 1);
}

/// Rewrite the runtime state hl-14's catalog entry records (a JSON value:
/// `"retired"`, or `null` for none) on the staged copy, and check the edit
/// took.
fn set_hl14_runtime_state(state: &Path, value: &str) {
    edit(
        state,
        "runtime.sqlite",
        &format!(
            "UPDATE runtime_session_catalog SET entry_json = CAST(replace(CAST(entry_json AS \
             TEXT), '\"runtime_state\":\"idle\"', '\"runtime_state\":{value}') AS BLOB) \
             WHERE session_id = '{KILLED_HL14_SESSION}'"
        ),
    );
    let entry: String = rusqlite::Connection::open(state.join("runtime.sqlite"))
        .expect("open runtime store")
        .query_row(
            "SELECT CAST(entry_json AS TEXT) FROM runtime_session_catalog WHERE session_id = ?1",
            [KILLED_HL14_SESSION],
            |row| row.get(0),
        )
        .expect("hl-14's catalog entry");
    assert!(
        entry.contains(&format!("\"runtime_state\":{value}")),
        "{entry}"
    );
}

/// A refused suffix row on hl-14's session whose remedy names no 0.9.3
/// operation, so not the restart.
fn assert_report_remedy(report: &NativeMigrateReport, identities: &[&str]) {
    assert_report_remedy_disagreeing(report, identities, &[]);
}

/// [`assert_report_remedy`] where the identity census also refuses the
/// identities in `disagreeing` (see [`assert_census_refusals_disagreeing`]).
fn assert_report_remedy_disagreeing(
    report: &NativeMigrateReport,
    identities: &[&str],
    disagreeing: &[&str],
) {
    let row = row_of(report, KILLED_HL14_SESSION);
    assert_eq!(row.class, SessionCensusClass::ProjectionSuffix, "{row:#?}");
    assert!(!row.admitted, "{row:#?}");
    assert_eq!(row.identities, identities, "{row:#?}");
    let action = row
        .action
        .as_deref()
        .expect("a refused row names an action");
    assert!(
        action.ends_with("keep the directory on 0.9.3 and report this census row"),
        "{action}"
    );
    assert!(!action.contains(HL14_RESTART_REMEDY), "{action}");
    assert_census_refusals_disagreeing(report, 1, disagreeing);
}

/// Control: a bound suffix row whose session 0.9.3 retired keeps the remedy
/// that names no 0.9.3 operation: 0.9.3 does not activate a retired session
/// at start.
#[tokio::test]
async fn a_bound_projection_suffix_on_a_retired_session_keeps_the_report_remedy() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Killed, temp.path());
    mint_hl14_suffix(&state).await;
    set_hl14_runtime_state(&state, "\"retired\"");
    assert_report_remedy(&census(&state).await, &["agent:hl-14"]);
}

/// Control: with no runtime state on the catalog entry the census cannot tell
/// that 0.9.3 left the session live, so the row keeps the report remedy.
#[tokio::test]
async fn a_bound_projection_suffix_with_no_recorded_state_keeps_the_report_remedy() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Killed, temp.path());
    mint_hl14_suffix(&state).await;
    set_hl14_runtime_state(&state, "null");
    assert_report_remedy(&census(&state).await, &["agent:hl-14"]);
}

/// Control: a suffix row two continuity records bind keeps the report
/// remedy: no restart of one identity is verified to converge it. The
/// twin's record names a session Core last bound to hl-14, which the
/// identity census refuses (R4).
#[tokio::test]
async fn a_projection_suffix_two_records_bind_keeps_the_report_remedy() {
    use released_0_9_3_h_local::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Killed, temp.path());
    mint_hl14_suffix(&state).await;
    edit(
        &state,
        "continuity.sqlite3",
        "CREATE TEMP TABLE twin AS SELECT * FROM continuity_records WHERE identity = \
         'agent:hl-14'; UPDATE twin SET identity = 'agent:hl-14-twin'; INSERT INTO \
         continuity_records SELECT * FROM twin;",
    );
    assert_report_remedy_disagreeing(
        &census(&state).await,
        &["agent:hl-14", "agent:hl-14-twin"],
        &["agent:hl-14-twin"],
    );
}

/// The identities each continuity record of a staged copy binds, by
/// session, read straight from the file (read-only).
fn recorded_bindings(state: &Path) -> BTreeMap<String, Vec<String>> {
    let connection = rusqlite::Connection::open_with_flags(
        state.join("continuity.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open the staged continuity copy read-only");
    let mut statement = connection
        .prepare("SELECT identity, session_id FROM continuity_records")
        .expect("prepare");
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("query");
    let mut bindings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows {
        let (identity, session) = row.expect("row");
        bindings.entry(session).or_default().push(identity);
    }
    bindings
}

/// Every released 0.9.3 H-app capture (an application that embeds the
/// runtime builder) reads as its plan:
/// - the roster arms are the builder layout: sessions.sqlite3 is the
///   projection and continuity.sqlite3 a real schema-1 file holding only the
///   records, with no head table; each arm reports the mob storage it used,
///   and the mob.sqlite3 arm is read once `--mob-store` names its store;
/// - the plain arm is a sessions projection;
/// - every committed session reads Equal, and each row's identities are
///   exactly the identities whose continuity record binds it.
#[tokio::test]
async fn every_released_h_app_capture_reads_as_its_plan() {
    use released_0_9_3_h_app::{Variant, stage_variant};
    for variant in Variant::ALL {
        let temp = tempfile::tempdir().expect("temp dir");
        let state = stage_variant(variant, temp.path());
        let name = variant.dir_name();
        let report = match variant.mob_store() {
            Some(store) if store != "event_log.sqlite3" => {
                census_naming(&state, &state.join(store)).await
            }
            _ => census(&state).await,
        };
        let (shape, mob_storage) = match variant.mob_store() {
            _ if !variant.has_roster() => (
                GatewayStateShape::SessionsProjection,
                CensusMobStore::InMemory,
            ),
            Some(store) => (
                GatewayStateShape::SessionsProjectionWithBindings,
                CensusMobStore::Durable {
                    path: state.join(store),
                },
            ),
            None => (
                GatewayStateShape::SessionsProjectionWithBindings,
                CensusMobStore::InMemory,
            ),
        };
        assert_eq!(report.shape, Some(shape), "{name}: {report:#?}");
        assert_eq!(report.mob_storage, Some(mob_storage), "{name}: {report:#?}");

        let bindings = if variant.has_roster() {
            let connection = rusqlite::Connection::open_with_flags(
                state.join("continuity.sqlite3"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .expect("open the staged continuity copy read-only");
            let version: i64 = connection
                .query_row(
                    "SELECT version FROM meerkat_schema WHERE domain = 'meta-continuity'",
                    [],
                    |row| row.get(0),
                )
                .expect("the continuity schema row");
            assert_eq!(version, 1, "{name}");
            let heads: bool = connection
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = \
                     'continuity_session_heads')",
                    [],
                    |row| row.get(0),
                )
                .expect("sqlite_master");
            assert!(!heads, "{name}: a schema-1 file has no head table");
            recorded_bindings(&state)
        } else {
            BTreeMap::new()
        };

        let manifest = variant.manifest();
        let committed: Vec<&String> = manifest["committed_sessions"]
            .as_object()
            .expect("committed sessions")
            .keys()
            .collect();
        assert_eq!(
            report.sessions.len(),
            committed.len(),
            "{name}: one row per committed session\n{report:#?}"
        );
        for session in committed {
            let row = row_of(&report, session);
            assert_eq!(row.class, SessionCensusClass::Equal, "{name}: {row:#?}");
            let mut bound = bindings.get(session.as_str()).cloned().unwrap_or_default();
            bound.sort();
            let mut listed = row.identities.clone();
            listed.sort();
            assert_eq!(listed, bound, "{name}: {row:#?}");
        }
        assert_only_census_refusals(&report, 0);
    }
}

/// Control: a builder directory whose continuity.sqlite3 also holds a
/// session body holds two projections, and nothing says which copy 0.9.3
/// served, so the census refuses it as ambiguous.
#[tokio::test]
async fn a_session_body_in_the_builders_continuity_file_is_ambiguous() {
    use released_0_9_3_h_app::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::Roster, temp.path());
    let session = session_of(&state, "app:app-01");
    edit(
        &state,
        "continuity.sqlite3",
        &format!(
            "INSERT INTO session_snapshots (session_id, identity, generation, \
             checkpoint_version, fencing_token, data) VALUES ('{session}', 'app:app-01', 0, 1, \
             1, x'00')"
        ),
    );
    let report = census(&state).await;
    assert_eq!(report.shape, None, "{report:#?}");
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::AmbiguousProjection]
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");
}

/// The mob.sqlite3 arm kept its durable mob store under 0.9.3's own
/// non-gateway name. Its composition manifest says a mob store is there, so
/// the census refuses until `--mob-store` names the store, and then reads
/// it.
#[tokio::test]
async fn a_mob_store_under_another_name_is_read_only_once_named() {
    use released_0_9_3_h_app::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::RosterDurableMobSqlite, temp.path());
    let report = census(&state).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::MobStoreUnnamed { manifests }]
                if *manifests == ["mob.sqlite3.composition.json"]
        ),
        "{report:#?}"
    );
    assert_eq!(report.mob_storage, None, "{report:#?}");
    assert!(report.sessions.is_empty(), "{report:#?}");

    let store = state.join("mob.sqlite3");
    let report = census_naming(&state, &store).await;
    assert_eq!(
        report.mob_storage,
        Some(CensusMobStore::Durable { path: store }),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
}

/// The directory's own event_log.sqlite3 manifest without the store: 0.9.3
/// writes that manifest only beside a durable store, so the census refuses
/// typed instead of reading the mob as in memory, and runs nothing else.
#[tokio::test]
async fn the_own_mob_store_manifest_without_its_store_is_refused() {
    use released_0_9_3_h_app::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::RosterDurable, temp.path());
    assert!(
        state.join("event_log.sqlite3.composition.json").is_file(),
        "the capture keeps the store's manifest"
    );
    let own = state.join("event_log.sqlite3");
    for suffix in ["", "-wal", "-shm"] {
        let path = state.join(format!("event_log.sqlite3{suffix}"));
        if path.exists() {
            std::fs::remove_file(&path).expect("remove the store");
        }
    }
    let report = census(&state).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::MobStoreManifestWithoutStore { manifest, store }]
                if manifest == "event_log.sqlite3.composition.json" && *store == own
        ),
        "{report:#?}"
    );
    assert_eq!(report.mob_storage, None, "{report:#?}");
    assert!(report.sessions.is_empty(), "{report:#?}");
}

/// `--mob-store` cannot contradict the directory: a named file that does not
/// exist refuses, and so does a named store other than the directory's own
/// event_log.sqlite3 when that exists. Naming event_log.sqlite3 itself names
/// the directory's own store.
#[tokio::test]
async fn a_named_mob_store_that_contradicts_the_directory_is_refused() {
    use released_0_9_3_h_app::{Variant, stage_variant};
    let temp = tempfile::tempdir().expect("temp dir");
    let state = stage_variant(Variant::RosterDurable, temp.path());
    let own = state.join("event_log.sqlite3");

    let missing = temp.path().join("absent.sqlite3");
    let report = census_naming(&state, &missing).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::MobStoreMissing { path }] if *path == missing
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");

    let elsewhere = temp.path().join("elsewhere.sqlite3");
    std::fs::copy(&own, &elsewhere).expect("copy the store elsewhere");
    let report = census_naming(&state, &elsewhere).await;
    assert!(
        matches!(
            report.refusals.as_slice(),
            [NativeMigrateRefusal::MobStoreConflict { named, found }]
                if *named == elsewhere && *found == own
        ),
        "{report:#?}"
    );
    assert!(report.sessions.is_empty(), "{report:#?}");

    let report = census_naming(&state, &own).await;
    assert_eq!(
        report.mob_storage,
        Some(CensusMobStore::Durable { path: own }),
        "{report:#?}"
    );
    assert_only_census_refusals(&report, 0);
}

/// The released 0.9.3 remote-continuity captures
/// (tests/fixtures/released_0_9_3_h_remote).
fn h_remote(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/released_0_9_3_h_remote")
        .join(file)
}

/// Unpack a capture's export file into `dir`.
fn h_remote_export(capture: &str, dir: &Path) -> PathBuf {
    let path = dir.join(format!("{capture}-export.jsonl"));
    let source = std::fs::File::open(h_remote(&format!("{capture}-export.jsonl.gz")))
        .expect("open the export");
    let mut out = std::fs::File::create(&path).expect("create the export");
    std::io::copy(&mut flate2::read::GzDecoder::new(source), &mut out).expect("gunzip");
    path
}

/// Unpack a capture's state directory into `dest`.
fn h_remote_state(capture: &str, dest: &Path) {
    let file =
        std::fs::File::open(h_remote(&format!("{capture}-state.tar.gz"))).expect("open the state");
    tar::Archive::new(flate2::read::GzDecoder::new(file))
        .unpack(dest)
        .expect("unpack the state");
}

fn export_request(state_dir: &Path, export: &Path, mode: MigrateMode) -> NativeMigrateRequest {
    NativeMigrateRequest::new(state_dir, mode).with_continuity_export(export)
}

/// The session each ready entry of an export carries, by session id.
fn exported_sessions(export: &Path) -> BTreeMap<String, meerkat_core::Session> {
    use base64::Engine as _;
    let text = std::fs::read_to_string(export).expect("read the export");
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).expect("entry");
            let session_id = entry.get("record")?["session_id"].as_str()?.to_owned();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(entry["document"]["base64"].as_str()?)
                .expect("document base64");
            let session = meerkat_core::Session::from_persisted_bytes(&bytes).expect("session");
            Some((session_id, session))
        })
        .collect()
}

fn identity_classes(report: &NativeMigrateReport) -> Vec<(&str, IdentityCensusClass)> {
    report
        .identities
        .iter()
        .map(|row| (row.identity.as_str(), row.class))
        .collect()
}

/// The file each pre-apply backup copies (its name up to `.pre-`), sorted.
fn backed_up_files(backups: &[PathBuf]) -> Vec<String> {
    let mut files: Vec<String> = backups
        .iter()
        .map(|backup| {
            let name = backup
                .file_name()
                .and_then(|name| name.to_str())
                .expect("a backup file name");
            name.split_once(".pre-")
                .map_or(name, |(file, _)| file)
                .to_owned()
        })
        .collect();
    files.sort();
    files
}

/// Each listed session's transcript in the migrated directory's session
/// store is the exported one.
async fn assert_imported_transcripts(state_dir: &Path, export: &Path, sessions: &[&str]) {
    let exported = exported_sessions(export);
    let native = meerkat_store::SqliteSessionStore::open(state_dir.join("runtime.sqlite"))
        .expect("native session store");
    for session_id in sessions {
        let id = meerkat_core::types::SessionId::parse(session_id).expect("session id");
        let imported = native
            .load(&id)
            .await
            .expect("load")
            .unwrap_or_else(|| panic!("{session_id} is in the native store"));
        let expected = &exported[*session_id];
        assert_eq!(
            meerkat_core::transcript_messages_digest(imported.messages()).expect("digest"),
            meerkat_core::transcript_messages_digest(expected.messages()).expect("digest"),
            "{session_id}"
        );
    }
}

const HR_MEMORY_ALPHA: &str = "01a15475-d66e-7181-8cfe-b1d7e051f719";
const HR_MEMORY_BIG: &str = "01a15475-d670-7ff3-9ef8-07fc26277e3c";
const HR_MEMORY_RESET: &str = "01a15475-f211-7732-a83b-4ba82ce1918b";
const HR_MEMORY_UNROSTERED: &str = "01a15475-d6f6-7411-9f74-35df595a3e8a";

/// An export into a new directory: every ready entry is a session Core does
/// not hold, imported through I1-I4 after the journal creates the native
/// pair, and each ready identity resumes through a first-boot Resume row
/// (in-memory rules: no mob storage). The uninitialized entries carry
/// nothing. A rerun finds a directory 0.9.4 already uses and refuses.
#[tokio::test]
async fn a_remote_export_imports_into_a_new_directory() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = h_remote_export("memory", scratch.path());
    let state = scratch.path().join("state");

    let census =
        migrate_state_dir_native(&export_request(&state, &export, MigrateMode::DryRun)).await;
    assert!(census.refusals.is_empty(), "{census:#?}");
    assert!(!state.exists(), "a dry run creates nothing");
    assert_eq!(census.shape, Some(GatewayStateShape::ContinuityExportNew));
    assert_eq!(census.mob_storage, Some(CensusMobStore::InMemory));
    let summary = census.continuity_export.as_ref().expect("export summary");
    assert_eq!((summary.entries, summary.ready), (6, 4));
    let mut missing: Vec<&str> = census
        .sessions
        .iter()
        .map(|row| {
            assert_eq!(row.class, SessionCensusClass::RuntimeMissing, "{row:#?}");
            row.session_id.as_str()
        })
        .collect();
    missing.sort_unstable();
    let mut expected = vec![
        HR_MEMORY_ALPHA,
        HR_MEMORY_BIG,
        HR_MEMORY_RESET,
        HR_MEMORY_UNROSTERED,
    ];
    expected.sort_unstable();
    assert_eq!(missing, expected);
    assert_eq!(
        identity_classes(&census),
        [
            ("agent:hr-alpha", IdentityCensusClass::Resume),
            ("agent:hr-big", IdentityCensusClass::Resume),
            ("agent:hr-deleted", IdentityCensusClass::Unbound),
            ("agent:hr-never", IdentityCensusClass::Unbound),
            ("agent:hr-reset", IdentityCensusClass::Resume),
            ("agent:hr-unrostered", IdentityCensusClass::Resume),
        ],
        "{census:#?}"
    );
    assert_eq!(census.first_boot.len(), 4, "{census:#?}");

    let report =
        migrate_state_dir_native(&export_request(&state, &export, MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    let applied = report.applied.as_ref().expect("applied");
    assert_eq!(applied.imported.len(), 4, "{applied:#?}");
    assert!(
        applied
            .imported
            .iter()
            .all(|import| import.row == ImportedRow::Created
                && import.activation == ImportedActivation::Installed),
        "{applied:#?}"
    );
    // A new directory backs up nothing; --apply keeps the byte copy of the
    // first-boot manifest it writes, as on every run.
    assert_eq!(
        backed_up_files(&applied.backups),
        ["storage-migrate-native.first-boot.json"],
        "{applied:#?}"
    );
    assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);
    let manifest = meerkat_meta::identity_first::FirstBootManifest::read(&state)
        .expect("manifest reads")
        .expect("manifest written");
    assert_eq!(manifest.rows().len(), 4);
    assert_imported_transcripts(&state, &export, &expected).await;

    let again =
        migrate_state_dir_native(&export_request(&state, &export, MigrateMode::Apply)).await;
    assert!(
        matches!(
            again.refusals.as_slice(),
            [NativeMigrateRefusal::ContinuityExportDirectory { .. }]
        ),
        "{again:#?}"
    );
    assert!(again.applied.is_none());
}

/// An export into the host's own directory, whose new volume holds only
/// some of the sessions: the export is the projection side, the sessions
/// both hold compare Equal, the two only the export holds are imported, and
/// the runtime-only bodies (no record names them) stay Core's.
#[tokio::test]
async fn a_remote_export_imports_what_the_host_directory_lacks() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = h_remote_export("memory", scratch.path());
    let state = scratch.path().join("state");
    std::fs::create_dir(&state).expect("state dir");
    h_remote_state("memory", &state);

    let report =
        migrate_state_dir_native(&export_request(&state, &export, MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    assert_eq!(
        report.shape,
        Some(GatewayStateShape::ContinuityExportInPlace)
    );
    let classes: BTreeMap<&str, SessionCensusClass> = report
        .sessions
        .iter()
        .map(|row| (row.session_id.as_str(), row.class))
        .collect();
    assert_eq!(classes[HR_MEMORY_ALPHA], SessionCensusClass::Equal);
    assert_eq!(classes[HR_MEMORY_RESET], SessionCensusClass::Equal);
    assert_eq!(classes[HR_MEMORY_BIG], SessionCensusClass::RuntimeMissing);
    assert_eq!(
        classes[HR_MEMORY_UNROSTERED],
        SessionCensusClass::RuntimeMissing
    );
    assert_eq!(
        classes
            .values()
            .filter(|class| **class == SessionCensusClass::ProjectionMissing)
            .count(),
        3,
        "{report:#?}"
    );
    let applied = report.applied.as_ref().expect("applied");
    let mut imported: Vec<&str> = applied
        .imported
        .iter()
        .map(|import| import.session_id.as_str())
        .collect();
    imported.sort_unstable();
    let mut expected = vec![HR_MEMORY_BIG, HR_MEMORY_UNROSTERED];
    expected.sort_unstable();
    assert_eq!(imported, expected, "{applied:#?}");
    assert_eq!(
        backed_up_files(&applied.backups),
        ["runtime.sqlite", "storage-migrate-native.first-boot.json"],
        "runtime.sqlite is backed up, and the first-boot manifest's copy kept: {applied:#?}"
    );
    assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);
    assert_imported_transcripts(
        &state,
        &export,
        &[
            HR_MEMORY_ALPHA,
            HR_MEMORY_BIG,
            HR_MEMORY_RESET,
            HR_MEMORY_UNROSTERED,
        ],
    )
    .await;
}

/// The host kept durable mob storage: each ready identity's record agrees
/// with Core's last binding (`hr-reset` on its third session), the identity
/// 0.9.3 deleted gets a Fresh row on the session its retired binding names,
/// and the identity that never existed carries nothing. Every session the
/// export lists is in runtime.sqlite already, so nothing is imported.
#[tokio::test]
async fn a_remote_export_with_durable_mob_storage_migrates_in_place() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = h_remote_export("durable", scratch.path());
    let state = scratch.path().join("state");
    std::fs::create_dir(&state).expect("state dir");
    h_remote_state("durable", &state);

    let report =
        migrate_state_dir_native(&export_request(&state, &export, MigrateMode::Apply)).await;
    assert!(report.refusals.is_empty(), "{report:#?}");
    assert!(
        matches!(report.mob_storage, Some(CensusMobStore::Durable { .. })),
        "{report:#?}"
    );
    let classes: BTreeMap<&str, SessionCensusClass> = report
        .sessions
        .iter()
        .map(|row| (row.session_id.as_str(), row.class))
        .collect();
    for session in [
        "01a15476-04af-7a82-8f04-a257b7e5f9d5",
        "01a15476-04b0-7ad3-af9f-e2cc0b6f5d08",
        "01a15476-25b0-7cf0-840b-d7da2caa47d2",
    ] {
        assert_eq!(classes[session], SessionCensusClass::Equal, "{session}");
    }
    // The same messages; the runtime copy's metadata is a later boundary's.
    assert!(
        matches!(
            classes["01a15476-0550-75d1-9765-65bd33580d3f"],
            SessionCensusClass::Equal | SessionCensusClass::ProjectionBehind
        ),
        "{report:#?}"
    );
    for session in [
        "01a15476-04b2-7e13-a236-f8a8da7b5314",
        "01a15476-04b3-7722-bf70-704c6bff3e0b",
        "01a15476-2488-7522-8873-2bbd17ca6046",
    ] {
        assert_eq!(
            classes[session],
            SessionCensusClass::ProjectionMissing,
            "{session}"
        );
    }
    assert_eq!(
        identity_classes(&report),
        [
            ("agent:hr-alpha", IdentityCensusClass::Bound),
            ("agent:hr-big", IdentityCensusClass::Bound),
            ("agent:hr-deleted", IdentityCensusClass::FreshAfterDelete),
            ("agent:hr-never", IdentityCensusClass::Unbound),
            ("agent:hr-reset", IdentityCensusClass::Bound),
            ("agent:hr-unrostered", IdentityCensusClass::Bound),
        ],
        "{report:#?}"
    );
    let resume = |identity: &str, session: &str| FirstBootRowPlan::Resume {
        identity: identity.to_owned(),
        session: session.to_owned(),
    };
    assert_eq!(
        report.first_boot,
        [
            resume("agent:hr-alpha", "01a15476-04af-7a82-8f04-a257b7e5f9d5"),
            resume("agent:hr-big", "01a15476-04b0-7ad3-af9f-e2cc0b6f5d08"),
            resume("agent:hr-reset", "01a15476-25b0-7cf0-840b-d7da2caa47d2"),
            resume(
                "agent:hr-unrostered",
                "01a15476-0550-75d1-9765-65bd33580d3f"
            ),
            FirstBootRowPlan::Fresh {
                identity: "agent:hr-deleted".to_owned(),
                retired_session: "01a15476-04b3-7722-bf70-704c6bff3e0b".to_owned(),
            },
        ]
    );
    let applied = report.applied.as_ref().expect("applied");
    assert!(applied.imported.is_empty(), "{applied:#?}");
    assert!(applied.first_boot_manifest_sha256.is_some(), "{applied:#?}");
    assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);
}

/// An unfinished export run whose journal this build cannot read, over a
/// directory Core never claimed: a dry run refuses MigrationIncomplete, and
/// --apply drops the journal and starts over with a new census, as a local
/// run does (only the claim makes a journal worth finishing).
#[tokio::test]
async fn an_unreadable_export_journal_without_the_claim_starts_over() {
    let scratch = tempfile::tempdir().expect("scratch");
    let export = h_remote_export("memory", scratch.path());
    let state = scratch.path().join("state");
    std::fs::create_dir(&state).expect("state dir");
    std::fs::write(state.join("storage-migrate-native.journal.json"), b"{}").expect("journal");

    let dry = migrate_state_dir_native(&export_request(&state, &export, MigrateMode::DryRun)).await;
    assert!(
        matches!(
            dry.refusals.as_slice(),
            [NativeMigrateRefusal::MigrationIncomplete { .. }]
        ),
        "{dry:#?}"
    );
    let apply =
        migrate_state_dir_native(&export_request(&state, &export, MigrateMode::Apply)).await;
    assert!(apply.refusals.is_empty(), "{apply:#?}");
    let applied = apply.applied.as_ref().expect("applied");
    assert!(applied.restarted && !applied.resumed, "{applied:#?}");
    assert_eq!(applied.imported.len(), 4, "{applied:#?}");
    assert_eq!(native_layout_status(&state), NativeLayoutStatus::Native);
}

/// Every file of a directory with its sha256: sidecars, lock files and
/// markers included, so any write shows.
fn every_file(dir: &Path) -> BTreeMap<PathBuf, String> {
    walk(dir)
        .into_iter()
        .map(|path| {
            let bytes = std::fs::read(&path).expect("read file");
            (
                path.strip_prefix(dir).expect("inside dir").to_path_buf(),
                format!("{:x}", Sha256::digest(&bytes)),
            )
        })
        .collect()
}

/// The offline identity census reads a native directory and writes
/// nothing in it: a directory that still needs the migration is refused,
/// naming the migration; the migrated one reads each identity's last Core
/// binding (session, generation, mob member id, Meta's runtime id while
/// live), the runtime id under which runtime.sqlite holds the session, the
/// catalog's terminal fact, the held completion owed (a mark without a
/// binding; a bound identity's mark owes nothing) and the pending
/// first-boot row; the JSON carries exactly the documented fields; no file
/// of the directory changes, not even a sidecar; and a held Meta runtime
/// lock refuses.
#[tokio::test]
async fn the_offline_identity_census_reads_a_native_directory_and_writes_nothing() {
    let dir = unpack("s1-roster");
    let unmigrated = identity_census_offline(dir.path(), None).await;
    match &unmigrated.refusal {
        Some(IdentityCensusRefusal::NeedsMigration { reasons, remedy }) => {
            assert!(!reasons.is_empty(), "{unmigrated:#?}");
            assert!(remedy.contains("storage-migrate-native"), "{remedy}");
        }
        other => panic!("an unmigrated directory needs the migration: {other:?}"),
    }
    assert!(unmigrated.identities.is_empty(), "{unmigrated:#?}");

    let applied =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(applied.refusals.is_empty(), "{applied:#?}");
    // The run kept its registered backups beside the databases; the census
    // copy leaves them out and still reads the directory as native.
    assert!(
        std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .any(
                |entry| meerkat_meta::storage_migrate::is_registered_backup_artifact_name(
                    &entry.file_name().to_string_lossy()
                )
            ),
        "the applied directory keeps its backups"
    );
    // Held-completion marks, written the way a held pass writes them: a
    // file named by the SHA-256 of the identity, holding the identity. One
    // for k1-held, which Core holds no binding of, so it owes its host a
    // held completion; one for k1-beta, which Core binds, so its mark is
    // one a published member left behind and it owes nothing.
    let marks = dir
        .path()
        .join(meerkat_meta::identity_first::HELD_COMPLETION_OWED_DIR);
    std::fs::create_dir(&marks).expect("marks dir");
    for identity in ["agent:k1-held", "agent:k1-beta"] {
        std::fs::write(
            marks.join(format!("{:x}", Sha256::digest(identity.as_bytes()))),
            identity,
        )
        .expect("mark");
    }

    let before = every_file(dir.path());
    let report = identity_census_offline(dir.path(), None).await;
    assert_eq!(report.refusal, None, "{report:#?}");
    assert_eq!(report.format, "meerkat-meta.identity-census");
    assert_eq!(report.version, 1);
    assert_eq!(
        report.mob_store,
        Some(dir.path().join("event_log.sqlite3")),
        "{report:#?}"
    );
    assert_eq!(report.mob_store_note, None);
    let names: Vec<&str> = report
        .identities
        .iter()
        .map(|entry| entry.identity.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "agent:k1-alpha",
            "agent:k1-beta",
            "agent:k1-gamma",
            "agent:k1-held"
        ]
    );
    let alpha = &report.identities[0];
    let session = "01a15366-b8d9-7213-970f-da32f39f4b42";
    assert_eq!(alpha.session_id.as_deref(), Some(session));
    assert_eq!(alpha.generation, Some(0));
    assert_eq!(alpha.member_id.as_deref(), Some("mk--agent_ck1-alpha"));
    assert!(alpha.live, "{alpha:#?}");
    assert!(alpha.agent_runtime_id.is_some(), "{alpha:#?}");
    // Under the session's runtime id or its legacy alias, whichever the
    // converted store holds the authority under.
    let session_runtime = format!("rt:session:{session}");
    assert!(
        matches!(
            alpha.runtime_id.as_deref(),
            Some(id) if id == session_runtime || id == session
        ),
        "{alpha:#?}"
    );
    assert!(!alpha.retired, "{alpha:#?}");
    assert!(!alpha.held_completion_owed, "{alpha:#?}");
    assert_eq!(
        serde_json::to_value(&alpha.pending_first_boot_row).expect("row json"),
        serde_json::json!({"kind": "resume", "identity": "agent:k1-alpha", "session": session})
    );
    assert!(report.identities[1].live, "{report:#?}");
    assert!(!report.identities[1].held_completion_owed, "{report:#?}");
    assert!(!report.identities[2].held_completion_owed, "{report:#?}");
    let held = &report.identities[3];
    assert!(held.held_completion_owed, "{held:#?}");
    assert_eq!(held.session_id, None, "{held:#?}");
    assert_eq!(held.member_id, None, "{held:#?}");
    assert!(!held.live, "{held:#?}");
    assert_eq!(held.runtime_id, None, "{held:#?}");
    assert_eq!(held.pending_first_boot_row, None, "{held:#?}");

    let json = serde_json::to_value(&report).expect("report json");
    let keys = |value: &serde_json::Value| {
        let mut keys: Vec<String> = value
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        keys.sort_unstable();
        keys
    };
    assert_eq!(
        keys(&json),
        [
            "format",
            "identities",
            "mob_store",
            "mob_store_note",
            "refusal",
            "state_dir",
            "version"
        ]
    );
    assert_eq!(
        keys(&json["identities"][0]),
        [
            "agent_runtime_id",
            "generation",
            "held_completion_owed",
            "identity",
            "live",
            "member_id",
            "pending_first_boot_row",
            "retired",
            "runtime_id",
            "session_id"
        ]
    );
    assert_eq!(every_file(dir.path()), before, "the census wrote nothing");

    let lock = std::fs::File::create(dir.path().join("meta-runtime.lock")).expect("lock file");
    lock.try_lock().expect("hold the lock");
    let held = identity_census_offline(dir.path(), None).await;
    assert!(
        matches!(
            held.refusal,
            Some(IdentityCensusRefusal::RuntimeInUse { .. })
        ),
        "{held:#?}"
    );
}

/// In-memory mob storage keeps no binding across a restart, so the census
/// still names every identity the directory knows (here each one the
/// continuity records bind, with its pending first-boot row) and leaves
/// every binding fact null, saying why at the top of the report.
#[tokio::test]
async fn the_offline_identity_census_names_in_memory_identities_with_null_facts() {
    let dir = unpack("s1e-memory-mob");
    let applied =
        migrate_state_dir_native(&NativeMigrateRequest::new(dir.path(), MigrateMode::Apply)).await;
    assert!(applied.refusals.is_empty(), "{applied:#?}");
    let manifest = meerkat_meta::identity_first::FirstBootManifest::read(dir.path())
        .expect("manifest reads")
        .expect("manifest written");
    assert!(!manifest.rows().is_empty());

    let report = identity_census_offline(dir.path(), None).await;
    assert_eq!(report.refusal, None, "{report:#?}");
    assert_eq!(report.mob_store, None);
    assert_eq!(
        report.mob_store_note,
        Some(meerkat_meta::storage_migrate_native::IN_MEMORY_MOB_STORE_NOTE)
    );
    for row in manifest.rows() {
        let entry = report
            .identities
            .iter()
            .find(|entry| entry.identity == row.identity().as_str())
            .unwrap_or_else(|| panic!("{} is listed: {report:#?}", row.identity()));
        assert_eq!(entry.session_id, None, "{entry:#?}");
        assert_eq!(entry.generation, None, "{entry:#?}");
        assert_eq!(entry.member_id, None, "{entry:#?}");
        assert!(!entry.live, "{entry:#?}");
        assert_eq!(entry.agent_runtime_id, None, "{entry:#?}");
        assert_eq!(entry.runtime_id, None, "{entry:#?}");
        assert!(!entry.retired, "{entry:#?}");
        assert!(!entry.held_completion_owed, "{entry:#?}");
        assert_eq!(
            serde_json::to_value(&entry.pending_first_boot_row).expect("row json"),
            serde_json::json!({
                "kind": "resume",
                "identity": row.identity().as_str(),
                "session": row.session().to_string(),
            })
        );
    }
    let json = serde_json::to_value(&report).expect("report json");
    assert_eq!(json["mob_store"], serde_json::Value::Null);
    assert!(json["mob_store_note"].is_string(), "{json:#}");
}
