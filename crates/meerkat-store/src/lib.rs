//! meerkat-store - Session persistence for Meerkat
//!
//! This crate provides storage backends for persisting conversation sessions.

// On wasm32, use tokio_with_wasm as a drop-in replacement for tokio.
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use meerkat_core::time_compat::wasm as time;
    pub use meerkat_core::tokio::spawn;
    pub use meerkat_core::tokio::task;
    pub use tokio_with_wasm::alias::*;
}

pub mod adapter;
#[cfg(not(target_arch = "wasm32"))]
pub mod approval_decision_store;
#[cfg(not(target_arch = "wasm32"))]
pub mod approval_file_store;
pub mod artifact;
pub mod blob;
#[cfg(not(target_arch = "wasm32"))]
pub mod doctor;
mod error;

#[cfg(not(target_arch = "wasm32"))]
pub mod index;
#[cfg(not(target_arch = "wasm32"))]
pub mod json_column;
#[cfg(not(target_arch = "wasm32"))]
pub mod migrate;
#[cfg(not(target_arch = "wasm32"))]
pub mod realm;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod schedule_sqlite_store;

#[cfg(all(feature = "jsonl", not(target_arch = "wasm32")))]
pub mod jsonl;

#[cfg(feature = "memory")]
pub mod memory;

#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub mod sqlite_store;

pub use adapter::StoreAdapter;
#[cfg(not(target_arch = "wasm32"))]
pub use approval_decision_store::SqliteDecisionStore;
#[cfg(not(target_arch = "wasm32"))]
pub use approval_file_store::FileApprovalStore;
pub use artifact::MemoryArtifactStore;
pub use blob::MemoryBlobStore;
#[cfg(not(target_arch = "wasm32"))]
pub use doctor::{DiskStorageMigrator, diagnose_disk_roots};
pub use error::StoreError;
#[cfg(not(target_arch = "wasm32"))]
pub use migrate::{
    MigrateMode, MigrateReport, PruneReport, RealmMaintenanceFence, archive_path_read_only,
    backup_artifact_name,
};

// Re-export the canonical trait, filter, and error from meerkat-core.
// Custom storage backends depend only on meerkat-core; existing consumers
// of meerkat-store see no change.
pub use meerkat_core::session_store::{
    HeadCanonicalAuthorityCrossing, HeadCanonicalStoreActivation, VerifiedHeadCanonicalAuthority,
};
pub use meerkat_core::{
    IncrementalSessionStore, RetainedStrandLayout, SessionFilter, SessionHead, SessionHeadCas,
    SessionStore, SessionStoreError, TranscriptStrandId, head_canonical_plain_save_guard,
    session_head_cas_token, strand_layout_for_history, strand_layout_for_retained_history,
};

#[cfg(not(target_arch = "wasm32"))]
pub use artifact::FsArtifactStore;
#[cfg(not(target_arch = "wasm32"))]
pub use blob::FsBlobStore;
#[cfg(not(target_arch = "wasm32"))]
pub use realm::FilesystemRealmConfigSource;
#[cfg(not(target_arch = "wasm32"))]
#[allow(deprecated)]
// the ambient no-`_in` wrappers stay exported through their deprecation window
pub use realm::{
    ExternalRealmManifest, GOVERNED_MANIFEST_FORMAT, RealmGovernance, RealmManifestPin,
    RealmPosture, SUPPORTED_MANIFEST_FORMAT, ensure_realm_manifest_pin_in, govern_realm_in,
    read_realm_manifest_pin, read_realm_posture_in,
};
#[cfg(not(target_arch = "wasm32"))]
#[allow(deprecated)]
// the ambient no-`_in` wrappers stay exported through their deprecation window
pub use realm::{
    GRANT_OWNER_STORE_RELATIVE_PATH, REALM_LEASE_HEARTBEAT_SECS, REALM_LEASE_STALE_TTL_SECS,
    RealmBackend, RealmLeaseGuard, RealmLeaseRecord, RealmLeaseStatus, RealmManifest,
    RealmManifestEntry, RealmOrigin, RealmPaths, derive_workspace_realm_id, ensure_realm_manifest,
    ensure_realm_manifest_in, fnv1a64_hex, generate_realm_id, grant_owner_store_path,
    inspect_realm_leases, inspect_realm_leases_in, list_realm_manifests_in,
    open_realm_session_store, open_realm_session_store_in, realm_lease_dir, realm_paths,
    realm_paths_in, sanitize_realm_id, start_realm_lease, start_realm_lease_in,
};
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub use schedule_sqlite_store::SqliteScheduleStore;
#[cfg(all(feature = "sqlite", not(target_arch = "wasm32")))]
pub use sqlite_store::{SqliteConnectionOptions, SqliteSessionStore};

#[cfg(all(feature = "jsonl", not(target_arch = "wasm32")))]
pub use jsonl::JsonlStore;

#[cfg(feature = "memory")]
pub use memory::{MemoryStore, MemoryStoreStats};

/// Test support for the rows on a store write the operating system refuses:
/// a scenario run in a process where every write to a regular file fails.
#[cfg(all(test, unix))]
#[allow(clippy::expect_used)]
pub(crate) mod refused_writes_test_support {
    use std::path::{Path, PathBuf};

    /// Set, to the directory the scenario works in, in the child process a
    /// scenario runs in.
    const REFUSED_CHILD: &str = "MEERKAT_STORE_TEST_WRITES_REFUSED_CHILD";
    /// Printed by that child once the scenario ran to its end.
    const REFUSED_CHILD_DONE: &str = "store scenario ran with every file write refused";
    /// Printed by that child instead, when a probe write succeeded.
    const REFUSED_CHILD_NOT_REFUSED: &str =
        "NOT exercised: this platform did not refuse a write past a zero file size limit";

    /// Run `scenario`, part of the test `test` in the module `module` (the
    /// caller's `module_path!()`), in a process that can create, rename, lock
    /// and read files but whose every write of at least one byte to a regular
    /// file fails (EFBIG): a zero file size limit with `SIGXFSZ` ignored. Both
    /// are process-wide, so the shared test process never sets them: the
    /// scenario runs in a child process of this test binary, started by a
    /// shell that sets them before exec, that runs `test` alone.
    ///
    /// `prepare` runs first, in this process, where writes work: it stores
    /// whatever content the scenario must not destroy and returns what
    /// `verify` compares against. `verify` runs last, in this process again.
    /// All three get the same fresh directory.
    ///
    /// On Linux the limit always refuses the write, so a probe write that
    /// succeeded there means the recipe broke: the row FAILS, because a
    /// proof that did not run must not pass. Elsewhere, where a platform may
    /// not enforce the limit this way, the row is skipped, with a line on
    /// stderr that says so.
    pub(crate) fn with_writes_refused<T>(
        module: &str,
        test: &str,
        prepare: impl FnOnce(&Path) -> T,
        scenario: impl FnOnce(&Path),
        verify: impl FnOnce(&Path, T),
    ) {
        if let Some(root) = std::env::var_os(REFUSED_CHILD) {
            let root = PathBuf::from(root);
            if std::fs::write(root.join("probe").join("byte"), b"x").is_ok() {
                println!("\n{REFUSED_CHILD_NOT_REFUSED}");
                return;
            }
            scenario(&root.join("scenario"));
            println!("\n{REFUSED_CHILD_DONE}");
            return;
        }
        let root = tempfile::tempdir().expect("a temporary directory");
        let work = root.path().join("scenario");
        std::fs::create_dir(&work).expect("the scenario directory");
        std::fs::create_dir(root.path().join("probe")).expect("the probe directory");
        let before = prepare(&work);
        let binary = std::env::current_exe().expect("the test binary");
        let (_, module) = module
            .split_once("::")
            .expect("a module path within the crate");
        let test_path = format!("{module}::{test}");
        #[allow(clippy::disallowed_methods)] // the test re-runs its own binary
        let output = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "trap '' XFSZ && ulimit -f 0 && exec \"$@\"",
                "store-writes-refused",
            ])
            .arg(binary.as_os_str())
            .args([
                "--exact",
                test_path.as_str(),
                "--nocapture",
                "--test-threads=1",
            ])
            .env(REFUSED_CHILD, root.path())
            .output()
            .expect("start the test with writes refused");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let not_refused = output.status.success() && stdout.contains(REFUSED_CHILD_NOT_REFUSED);
        if not_refused && !cfg!(target_os = "linux") {
            eprintln!("SKIPPED {test}: {REFUSED_CHILD_NOT_REFUSED}");
            return;
        }
        assert!(
            !not_refused,
            "{test} proved nothing: {REFUSED_CHILD_NOT_REFUSED}, and Linux always does"
        );
        assert!(
            output.status.success() && stdout.contains(REFUSED_CHILD_DONE),
            "{test} failed with writes refused ({}):\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        verify(&work, before);
    }

    /// A current-thread runtime for one stage of a scenario.
    pub(crate) fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    }

    /// Every file under `dir`, as a sorted list of paths relative to it. A
    /// missing `dir` holds none.
    pub(crate) fn files_under(dir: &Path) -> Vec<String> {
        fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    out.push(
                        path.strip_prefix(base)
                            .expect("a path under the base")
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }
}
