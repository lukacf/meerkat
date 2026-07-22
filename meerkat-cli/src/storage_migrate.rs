//! `rkat storage migrate` / `rkat storage prune` orchestration (Phase 6 of
//! the storage unification arc).
//!
//! The reusable primitives (realm maintenance fence, backup naming, report
//! shapes, divergence computation) live in `meerkat_store::migrate`; this
//! module owns the disk-realm orchestration because only the CLI sits above
//! every store crate and can run each store's normal constructor.
//!
//! The five migration cases:
//!
//! 1. **Ledger baseline (auto-safe):** under the realm's exclusive
//!    maintenance fence, each store is opened through its NORMAL constructor
//!    — the guarded schema-ledger migrations ARE the structural
//!    verification, converging files of any vintage. Dry-run reads versions
//!    read-only and reports what would be stamped.
//! 2. **State-root adoption (report-only):** the dual-root resolver already
//!    uses realms where they lie; the report states each realm's root.
//! 3. **Split-brain reconciliation (manual, fail-closed):** a realm id under
//!    2+ swept roots produces a per-domain divergence report and a typed
//!    refusal. With `--apply --adopt-root <path>` one root is adopted where
//!    it lies and every other copy is archived read-only under the
//!    registered backup naming. No synthesis, no merging.
//! 4. **Checkpoint-evidence adoption:** for sqlite realms, the persistent
//!    session service (composed with the agent-refusing
//!    `MaintenanceAgentBuilder`) runs the machine-owned bulk
//!    `adopt_legacy_checkpoints` sweep under the fence (holder
//!    self-admission lets production store paths pass their per-operation
//!    guards). JSONL realms report "adoption skipped" — their sessions heal
//!    lazily on first authority touch.
//! 5. **Deprecated leftovers (report-only):** doctor's artifact findings
//!    plus a legacy `<home>/.rkat/sessions` directory if present.
//!    Credential stores are never read, moved, or reported.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use meerkat_core::storage_diagnostics::{DiagnoseScope, FindingSeverity, StorageFinding};
#[cfg(feature = "session-store")]
use meerkat_store::migrate::CheckpointAdoptionRefusal;
use meerkat_store::migrate::{
    self as store_migrate, CheckpointAdoptionOutcome, DivergenceStatus, LedgerBaselineAction,
    LedgerBaselineEntry, MigrateMode, MigrateReport, PruneAction, PruneReport, RealmDirEntry,
    RealmMigrateReport, SplitBrainReport, SplitBrainResolution,
};

/// Case-5 finding codes copied into the migrate report (deprecated
/// leftovers; report-only).
const LEFTOVER_FINDING_CODES: &[&str] = &[
    meerkat_store::doctor::FINDING_BACKUP_ARTIFACT,
    meerkat_store::doctor::FINDING_QUARANTINED_INDEX,
    meerkat_store::doctor::FINDING_ORPHANED_LEASE,
    meerkat_store::doctor::FINDING_UNPARSEABLE_LEASE,
    meerkat_store::doctor::FINDING_STALE_MANIFEST_LOCK,
];

/// Options for one `storage migrate` run. Roots are resolved by the caller
/// (explicit roots, or the CLI's dual-root candidates) — this module reads
/// nothing ambient.
pub(crate) struct MigrateOptions {
    pub roots: Vec<PathBuf>,
    pub realm_filter: Option<String>,
    pub apply: bool,
    pub adopt_root: Option<PathBuf>,
    pub fence_wait: Duration,
    /// Legacy pre-realm `<home>/.rkat/sessions` directory to probe
    /// (report-only), resolved by the caller's bootstrap.
    pub legacy_home_sessions: Option<PathBuf>,
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Enumerate realm directories across the swept roots, deduplicating roots
/// by canonical identity and applying the realm filter the same way doctor
/// does (sanitized directory name or manifest identity).
fn enumerate_realms(roots: &[PathBuf], realm_filter: Option<&str>) -> Vec<RealmDirEntry> {
    let mut seen_roots: Vec<PathBuf> = Vec::new();
    let mut realms = Vec::new();
    for root in roots {
        let root_canonical = canonical(root);
        if seen_roots.contains(&root_canonical) {
            continue;
        }
        seen_roots.push(root_canonical);
        for realm in store_migrate::list_realm_dirs(root) {
            if let Some(filter) = realm_filter {
                let dir_name = realm
                    .dir
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if dir_name != meerkat_core::sanitize_realm_id(filter) && realm.realm_id != filter {
                    continue;
                }
            }
            realms.push(realm);
        }
    }
    realms
}

/// Group realms by id, deduplicating same-directory spellings so one realm
/// reached through two root spellings never fabricates a twin.
fn group_by_realm(realms: Vec<RealmDirEntry>) -> BTreeMap<String, Vec<RealmDirEntry>> {
    let mut groups: BTreeMap<String, Vec<RealmDirEntry>> = BTreeMap::new();
    for realm in realms {
        let entry = groups.entry(realm.realm_id.clone()).or_default();
        let dir_canonical = canonical(&realm.dir);
        if !entry
            .iter()
            .any(|existing| canonical(&existing.dir) == dir_canonical)
        {
            entry.push(realm);
        }
    }
    for copies in groups.values_mut() {
        copies.sort_by(|a, b| a.dir.cmp(&b.dir));
    }
    groups
}

/// Run `storage migrate` over the given options and produce the report.
/// The caller renders it and maps [`MigrateReport::has_errors`] to exit 1.
pub(crate) async fn run_storage_migrate(options: MigrateOptions) -> MigrateReport {
    let mode = if options.apply {
        MigrateMode::Apply
    } else {
        MigrateMode::DryRun
    };
    let mut report = MigrateReport::new(mode, options.roots.clone());

    let mut groups = group_by_realm(enumerate_realms(
        &options.roots,
        options.realm_filter.as_deref(),
    ));

    // ── Case 3: split-brain reconciliation (fail-closed). ────────────────
    let twin_ids: Vec<String> = groups
        .iter()
        .filter(|(_, copies)| copies.len() > 1)
        .map(|(realm_id, _)| realm_id.clone())
        .collect();
    let resolving = options.apply && options.adopt_root.is_some();
    for realm_id in &twin_ids {
        let Some(copies) = groups.get(realm_id).cloned() else {
            continue;
        };
        let locations: Vec<PathBuf> = copies.iter().map(|copy| copy.dir.clone()).collect();
        let divergence_realm = realm_id.clone();
        let divergence_locations = locations.clone();
        let mut split = tokio::task::spawn_blocking(move || {
            store_migrate::compute_split_brain_report(&divergence_realm, &divergence_locations)
        })
        .await
        .unwrap_or_else(|join_error| {
            let mut failed = SplitBrainReport::new(realm_id.clone(), locations);
            failed
                .errors
                .push(format!("divergence computation failed: {join_error}"));
            failed
        });

        if let (true, Some(adopt_root)) = (options.apply, options.adopt_root.as_deref()) {
            match resolve_split_brain(&copies, adopt_root, options.fence_wait).await {
                Ok((adopted, archived)) => {
                    split.resolution = SplitBrainResolution::Archived {
                        adopted: adopted.dir.clone(),
                        archived,
                    };
                    groups.insert(realm_id.clone(), vec![adopted]);
                }
                Err(reason) => {
                    report
                        .errors
                        .push(format!("split-brain realm '{realm_id}': {reason}"));
                    split.resolution = SplitBrainResolution::Refused { reason };
                    groups.remove(realm_id);
                }
            }
        } else {
            report.errors.push(format!(
                "split-brain realm '{realm_id}' is materialized under multiple swept roots; \
                 fail-closed refusal — rerun with `--apply --adopt-root <path>` to adopt one \
                 root and archive the other copies read-only"
            ));
        }
        report.split_brain.push(split);
    }

    if !twin_ids.is_empty() && !resolving {
        // Fail-closed: the divergence report is the whole output.
        return report;
    }

    // ── Cases 1, 2, 4 per unique realm materialization. ──────────────────
    for copies in groups.values() {
        let Some(realm) = copies.first() else {
            continue;
        };
        if copies.len() > 1 {
            continue; // unresolved twin; already reported
        }
        report
            .realms
            .push(migrate_realm(realm, options.apply, options.fence_wait).await);
    }

    // ── Case 5: deprecated leftovers (report-only). ──────────────────────
    let mut scope = DiagnoseScope::new(options.roots.clone());
    if let Some(filter) = &options.realm_filter {
        scope = scope.with_realm(filter.clone());
    }
    let diagnosis = meerkat_store::diagnose_disk_roots(&scope).await;
    report.findings.extend(
        diagnosis
            .findings
            .into_iter()
            .filter(|finding| LEFTOVER_FINDING_CODES.contains(&finding.code.as_str())),
    );
    if let Some(legacy_dir) = &options.legacy_home_sessions
        && legacy_dir.is_dir()
    {
        report.findings.push(
            StorageFinding::new(
                FindingSeverity::Info,
                store_migrate::FINDING_LEGACY_HOME_SESSIONS_DIR,
                "legacy pre-realm sessions directory (report-only; migrate does not move it — \
                 realm-scoped storage supersedes it)",
            )
            .with_path(legacy_dir.clone()),
        );
    }

    report
}

/// Verify the adopted root is one of the twin's swept roots, then fence
/// every copy and archive the non-adopted copies read-only. Returns the
/// adopted copy and the archive paths.
async fn resolve_split_brain(
    copies: &[RealmDirEntry],
    adopt_root: &Path,
    fence_wait: Duration,
) -> Result<(RealmDirEntry, Vec<PathBuf>), String> {
    let adopt_canonical = canonical(adopt_root);
    let adopted = copies
        .iter()
        .find(|copy| canonical(&copy.state_root) == adopt_canonical)
        .cloned()
        .ok_or_else(|| {
            format!(
                "--adopt-root {} is not one of the swept roots materializing this realm \
                 (candidates: {})",
                adopt_root.display(),
                copies
                    .iter()
                    .map(|copy| copy.state_root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?;

    let adopted_dir = adopted.dir.clone();
    let copy_dirs: Vec<PathBuf> = copies.iter().map(|copy| copy.dir.clone()).collect();
    let archived = tokio::task::spawn_blocking(move || {
        // Fence BOTH (all) copies during the archive; copies arrive sorted,
        // so two racing migrators acquire in the same global order.
        let mut fences = Vec::with_capacity(copy_dirs.len());
        for dir in &copy_dirs {
            let fence = store_migrate::RealmMaintenanceFence::acquire(dir, fence_wait)
                .map_err(|error| format!("maintenance fence not acquirable: {error}"))?;
            fences.push(Some(fence));
        }
        let mut archived = Vec::new();
        for (index, dir) in copy_dirs.iter().enumerate() {
            if *dir == adopted_dir {
                continue;
            }
            // Release this copy's fence right before the rename: the fence
            // holds open lock-file handles inside the directory, and a
            // directory with open handles cannot be renamed on Windows.
            // Quiescence is already established; the fences on the adopted
            // copy stay held until every archive completes.
            fences[index] = None;
            let archive = store_migrate::archive_path_read_only(dir, "split-brain")
                .map_err(|error| format!("archive of {} failed: {error}", dir.display()))?;
            archived.push(archive);
        }
        drop(fences);
        Ok::<_, String>(archived)
    })
    .await
    .map_err(|join_error| format!("archive task failed: {join_error}"))??;

    Ok((adopted, archived))
}

/// Ledger versions for every inventoried database currently on disk:
/// `(database, domain) -> version` for ledger rows, `None` for expected
/// domains without a row. Read-only.
fn read_ledger_matrix(realm_dir: &Path) -> Vec<(PathBuf, String, Option<i64>)> {
    let mut matrix = Vec::new();
    for (relative, expected_domains) in meerkat_store::doctor::REALM_DATABASE_FILES {
        let db_path = realm_dir.join(relative);
        if !db_path.is_file() {
            continue;
        }
        let rows = store_migrate::read_domain_versions(&db_path)
            .ok()
            .flatten()
            .unwrap_or_default();
        for (domain, version) in &rows {
            matrix.push((db_path.clone(), domain.clone(), Some(*version)));
        }
        for expected in *expected_domains {
            if !rows.iter().any(|(domain, _)| domain == expected) {
                matrix.push((db_path.clone(), (*expected).to_string(), None));
            }
        }
    }
    matrix
}

/// Report-only entries for per-mob databases (`mobs/*.db`): their ledger
/// versions are read, but v1 never opens mob stores offline — the owning
/// store converges each file on its next open.
fn mob_report_only_entries(realm_dir: &Path, entry: &mut RealmMigrateReport) {
    let Ok(dir_entries) = std::fs::read_dir(realm_dir.join("mobs")) else {
        return;
    };
    let mut databases: Vec<PathBuf> = dir_entries
        .filter_map(Result::ok)
        .map(|dir_entry| dir_entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("db") && path.is_file()
        })
        .collect();
    databases.sort();
    if databases.is_empty() {
        return;
    }
    for database in databases {
        let version = store_migrate::read_domain_versions(&database)
            .ok()
            .flatten()
            .and_then(|rows| {
                rows.into_iter()
                    .find(|(domain, _)| domain == "mob")
                    .map(|(_, version)| version)
            });
        let mut ledger_entry =
            LedgerBaselineEntry::new(database, "mob", LedgerBaselineAction::ReportOnly);
        ledger_entry.before = version;
        entry.ledger.push(ledger_entry);
    }
    entry.notes.push(
        "mob databases are report-only in v1; the owning mob store converges each file on its \
         next open"
            .to_string(),
    );
}

/// Cases 1, 2, and 4 for one realm materialization.
async fn migrate_realm(
    realm: &RealmDirEntry,
    apply: bool,
    fence_wait: Duration,
) -> RealmMigrateReport {
    let mut entry = RealmMigrateReport::new(realm.realm_id.clone(), realm.dir.clone());
    entry.backend = realm.backend.clone();
    // Case 2: state-root adoption is report-only — the realm is used where
    // it lies (the phase-2 dual-root resolver's steady state).
    entry.notes.push(format!(
        "state-root adoption: realm is used where it lies (state root {})",
        realm.state_root.display()
    ));

    if !realm.manifest_readable {
        entry.errors.push(
            "realm manifest unreadable; refusing to migrate (repair or remove the manifest \
             first)"
                .to_string(),
        );
        return entry;
    }
    let backend = realm.backend.clone().unwrap_or_default();
    match backend.as_str() {
        "memory" => {
            entry
                .notes
                .push("memory backend: no durable storage; nothing to migrate".to_string());
            return entry;
        }
        "sqlite" | "jsonl" => {}
        other => {
            entry.notes.push(format!(
                "backend '{other}' is not disk-migratable by this build; report-only"
            ));
            for (database, domain, version) in read_ledger_matrix(&realm.dir) {
                let mut ledger_entry =
                    LedgerBaselineEntry::new(database, domain, LedgerBaselineAction::ReportOnly);
                ledger_entry.before = version;
                entry.ledger.push(ledger_entry);
            }
            return entry;
        }
    }

    // Path-alias guard: the realm's directory must be the one its id
    // sanitizes to, or the normal constructors would open a different path.
    let expected_dir = meerkat_store::realm_paths_in(&realm.state_root, &realm.realm_id).root;
    if canonical(&expected_dir) != canonical(&realm.dir) {
        entry.errors.push(format!(
            "realm id '{}' sanitizes to directory {}, but this realm lies at {}; refusing to \
             open (path-aliased manifest)",
            realm.realm_id,
            expected_dir.display(),
            realm.dir.display()
        ));
        return entry;
    }

    if !apply {
        dry_run_realm(realm, &backend, &mut entry).await;
        mob_report_only_entries(&realm.dir, &mut entry);
        return entry;
    }

    apply_realm(realm, &backend, fence_wait, &mut entry).await;
    mob_report_only_entries(&realm.dir, &mut entry);
    entry
}

/// Case 1 + 4 dry-run: read-only ledger matrix and legacy census. The
/// database bytes are untouched.
async fn dry_run_realm(realm: &RealmDirEntry, backend: &str, entry: &mut RealmMigrateReport) {
    let dir = realm.dir.clone();
    let matrix = tokio::task::spawn_blocking(move || read_ledger_matrix(&dir))
        .await
        .unwrap_or_default();
    for (database, domain, before) in matrix {
        let action = if before.is_some() {
            LedgerBaselineAction::Recorded
        } else {
            LedgerBaselineAction::WouldStamp
        };
        let mut ledger_entry = LedgerBaselineEntry::new(database, domain, action);
        ledger_entry.before = before;
        entry.ledger.push(ledger_entry);
    }

    match backend {
        "sqlite" => {
            let sessions_db = realm.dir.join("sessions.sqlite3");
            if sessions_db.is_file() {
                match tokio::task::spawn_blocking(move || {
                    store_migrate::census_legacy_sessions(&sessions_db)
                })
                .await
                {
                    Ok(Ok(census)) => {
                        let mut adoption = CheckpointAdoptionOutcome::default();
                        adoption.legacy_pending = census.legacy;
                        adoption.already_verified = census.verified;
                        adoption.skipped = Some(
                            "dry-run: census only; --apply runs the bulk adoption sweep"
                                .to_string(),
                        );
                        entry.adoption = Some(adoption);
                        if census.invalid > 0 {
                            entry.errors.push(format!(
                                "{} session document(s) carry malformed checkpoint metadata \
                                 (present-but-invalid evidence is never laundered into legacy)",
                                census.invalid
                            ));
                        }
                    }
                    Ok(Err(error)) => entry
                        .errors
                        .push(format!("legacy checkpoint census failed: {error}")),
                    Err(join_error) => entry
                        .errors
                        .push(format!("legacy checkpoint census failed: {join_error}")),
                }
            }
        }
        "jsonl" => {
            let mut adoption = CheckpointAdoptionOutcome::default();
            adoption.skipped = Some(
                "adoption skipped (jsonl backend): sessions heal lazily on first authority touch"
                    .to_string(),
            );
            entry.adoption = Some(adoption);
        }
        _ => {}
    }
}

/// Case 1 + 4 apply: fence the realm, run every store's normal constructor
/// (the guarded ledger migrations are the structural verification), then
/// run the machine-owned bulk checkpoint adoption for sqlite realms.
#[cfg(feature = "session-store")]
async fn apply_realm(
    realm: &RealmDirEntry,
    backend: &str,
    fence_wait: Duration,
    entry: &mut RealmMigrateReport,
) {
    // Exclusive maintenance fence over every database in the realm (waits
    // for in-flight per-operation guards to drain; foreign holders surface
    // typed).
    let fence_dir = realm.dir.clone();
    let fence = match tokio::task::spawn_blocking(move || {
        store_migrate::RealmMaintenanceFence::acquire(&fence_dir, fence_wait)
    })
    .await
    {
        Ok(Ok(fence)) => fence,
        Ok(Err(error)) => {
            entry
                .errors
                .push(format!("maintenance fence not acquirable: {error}"));
            return;
        }
        Err(join_error) => {
            entry
                .errors
                .push(format!("maintenance fence not acquirable: {join_error}"));
            return;
        }
    };

    let before_dir = realm.dir.clone();
    let before = tokio::task::spawn_blocking(move || read_ledger_matrix(&before_dir))
        .await
        .unwrap_or_default();

    // Case 1: normal constructors via the facade bundle (sessions, schedule,
    // runtime, workgraph, blobs) — plus the stores the bundle does not own.
    let opened =
        meerkat::open_realm_persistence_in(&realm.state_root, &realm.realm_id, None, None).await;
    let bundle = match opened {
        Ok((_manifest, bundle)) => bundle,
        Err(error) => {
            entry
                .errors
                .push(format!("realm store open failed: {error}"));
            drop(fence);
            return;
        }
    };

    let memory_db = realm.dir.join("memory").join("memory.sqlite3");
    if memory_db.is_file() {
        #[cfg(feature = "memory-store")]
        {
            let memory_dir = realm.dir.join("memory");
            match tokio::task::spawn_blocking(move || meerkat::HnswMemoryStore::open(&memory_dir))
                .await
            {
                Ok(Ok(_store)) => {}
                Ok(Err(error)) => entry
                    .errors
                    .push(format!("memory store open failed: {error}")),
                Err(join_error) => entry
                    .errors
                    .push(format!("memory store open failed: {join_error}")),
            }
        }
        #[cfg(not(feature = "memory-store"))]
        entry.notes.push(
            "memory database present but this build lacks the memory-store feature; report-only"
                .to_string(),
        );
    }
    let tasks_db = realm.dir.join("tasks.db");
    if tasks_db.is_file() {
        use meerkat::TaskStore as _;
        let task_store = meerkat::SqliteTaskStore::unscoped(&tasks_db);
        if let Err(error) = task_store.list().await {
            entry
                .errors
                .push(format!("task store open failed: {error}"));
        }
    }
    let index_db = realm
        .dir
        .join("sessions_jsonl")
        .join("session_index.sqlite3");
    if index_db.is_file()
        && let Err(error) = tokio::task::spawn_blocking(move || {
            meerkat_store::index::SqliteSessionIndex::open(index_db)
        })
        .await
        .map_err(|join_error| meerkat_store::StoreError::Internal(join_error.to_string()))
        .and_then(|result| result.map(|_| ()))
    {
        entry
            .errors
            .push(format!("session index open failed: {error}"));
    }

    // Case 4: machine-owned bulk checkpoint adoption (sqlite realms).
    if backend == "sqlite" {
        let (session_store, runtime_store, blob_store) = bundle.into_parts();
        let service = meerkat::PersistentSessionService::new(
            meerkat::MaintenanceAgentBuilder,
            1,
            session_store,
            runtime_store,
            blob_store,
        );
        match service.adopt_legacy_checkpoints().await {
            Ok(adoption) => {
                let refused: Vec<CheckpointAdoptionRefusal> = adoption
                    .refused
                    .iter()
                    .map(|(session_id, reason)| {
                        CheckpointAdoptionRefusal::new(session_id.to_string(), reason.clone())
                    })
                    .collect();
                if !refused.is_empty() {
                    entry.errors.push(format!(
                        "{} session(s) refused checkpoint adoption; see the adoption report",
                        refused.len()
                    ));
                }
                let mut outcome = CheckpointAdoptionOutcome::default();
                outcome.scanned = adoption.scanned;
                outcome.already_verified = adoption.already_verified;
                outcome.adopted = adoption.adopted;
                outcome.refused = refused;
                entry.adoption = Some(outcome);
            }
            Err(error) => entry
                .errors
                .push(format!("bulk checkpoint adoption failed: {error}")),
        }
    } else {
        let mut adoption = CheckpointAdoptionOutcome::default();
        adoption.skipped = Some(
            "adoption skipped (jsonl backend): sessions heal lazily on first authority touch"
                .to_string(),
        );
        entry.adoption = Some(adoption);
        drop(bundle);
    }

    // Ledger entries: before → after per database × domain.
    let after_dir = realm.dir.clone();
    let after = tokio::task::spawn_blocking(move || read_ledger_matrix(&after_dir))
        .await
        .unwrap_or_default();
    let before_of = |database: &Path, domain: &str| -> Option<i64> {
        before
            .iter()
            .find(|(db, dom, _)| db == database && dom == domain)
            .and_then(|(_, _, version)| *version)
    };
    for (database, domain, after_version) in after {
        let before_version = before_of(&database, &domain);
        let action = if after_version == before_version {
            LedgerBaselineAction::AlreadyCurrent
        } else {
            LedgerBaselineAction::Stamped
        };
        let mut ledger_entry = LedgerBaselineEntry::new(database, domain, action);
        ledger_entry.before = before_version;
        ledger_entry.after = after_version;
        entry.ledger.push(ledger_entry);
    }

    drop(fence);
}

#[cfg(not(feature = "session-store"))]
async fn apply_realm(
    _realm: &RealmDirEntry,
    _backend: &str,
    _fence_wait: Duration,
    entry: &mut RealmMigrateReport,
) {
    entry.errors.push(
        "this rkat build lacks the session-store feature; `storage migrate --apply` is \
         unavailable"
            .to_string(),
    );
}

// ─────────────────────────────────────────────────────────────────────────
// `storage prune`
// ─────────────────────────────────────────────────────────────────────────

/// Options for one `storage prune` run.
pub(crate) struct PruneOptions {
    pub roots: Vec<PathBuf>,
    pub apply: bool,
    pub older_than_days: u64,
}

/// Run `storage prune`: enumerate registered maintenance artifacts
/// (`*.pre-*` backups and `*.corrupt-*` quarantines) under the swept roots;
/// with `--apply`, delete those at least `older_than_days` old. Nothing
/// outside the registered naming patterns is ever touched.
pub(crate) async fn run_storage_prune(options: PruneOptions) -> PruneReport {
    let roots = options.roots.clone();
    let mut artifacts =
        tokio::task::spawn_blocking(move || store_migrate::enumerate_maintenance_artifacts(&roots))
            .await
            .unwrap_or_default();

    let mode = if options.apply {
        MigrateMode::Apply
    } else {
        MigrateMode::DryRun
    };
    let mut report = PruneReport::new(mode, options.roots, options.older_than_days);

    for artifact in &mut artifacts {
        if artifact.age_days < options.older_than_days {
            artifact.action = PruneAction::Kept;
            continue;
        }
        if !options.apply {
            artifact.action = PruneAction::WouldDelete;
            continue;
        }
        let path = artifact.path.clone();
        match tokio::task::spawn_blocking(move || store_migrate::remove_maintenance_artifact(&path))
            .await
        {
            Ok(Ok(())) => artifact.action = PruneAction::Deleted,
            Ok(Err(error)) => {
                artifact.action = PruneAction::DeleteFailed;
                report.errors.push(format!(
                    "failed to delete {}: {error}",
                    artifact.path.display()
                ));
            }
            Err(join_error) => {
                artifact.action = PruneAction::DeleteFailed;
                report.errors.push(format!(
                    "failed to delete {}: {join_error}",
                    artifact.path.display()
                ));
            }
        }
    }
    report.artifacts = artifacts;
    report
}

// ─────────────────────────────────────────────────────────────────────────
// Text rendering (per-realm grouping; --json serializes the report types).
// ─────────────────────────────────────────────────────────────────────────

fn describe_status(status: &DivergenceStatus) -> String {
    match status {
        DivergenceStatus::Equal => "equal".to_string(),
        DivergenceStatus::Divergent => "divergent".to_string(),
        DivergenceStatus::OnlyIn { location } => format!("only in {}", location.display()),
        _ => "unknown".to_string(),
    }
}

fn describe_ledger_action(action: LedgerBaselineAction) -> &'static str {
    match action {
        LedgerBaselineAction::WouldStamp => "would-stamp",
        LedgerBaselineAction::Recorded => "recorded",
        LedgerBaselineAction::Stamped => "stamped",
        LedgerBaselineAction::AlreadyCurrent => "already-current",
        LedgerBaselineAction::ReportOnly => "report-only",
        _ => "unknown",
    }
}

fn describe_version(version: Option<i64>) -> String {
    version.map_or_else(|| "none".to_string(), |value| value.to_string())
}

/// Human-readable migrate report, grouped per realm.
pub(crate) fn print_migrate_report_text(report: &MigrateReport) {
    let mode = match report.mode {
        MigrateMode::Apply => "apply",
        _ => "dry-run",
    };
    println!(
        "Storage migrate ({mode}) over {} root(s):",
        report.swept_roots.len()
    );
    for root in &report.swept_roots {
        println!("  {}", root.display());
    }
    println!();

    for split in &report.split_brain {
        println!("Split-brain realm '{}':", split.realm);
        for location in &split.locations {
            println!("  copy: {}", location.display());
        }
        println!(
            "  sessions: {} equal across all copies",
            split.sessions_equal
        );
        for session in &split.sessions {
            println!(
                "    {}: {}",
                session.session_id,
                describe_status(&session.status)
            );
        }
        for file in &split.files {
            println!("  file {}: {}", file.file, describe_status(&file.status));
        }
        match &split.resolution {
            SplitBrainResolution::Refused { reason } => {
                println!("  resolution: REFUSED — {reason}");
            }
            SplitBrainResolution::Archived { adopted, archived } => {
                println!("  resolution: adopted {}", adopted.display());
                for archive in archived {
                    println!("    archived read-only: {}", archive.display());
                }
            }
            _ => println!("  resolution: unknown"),
        }
        for error in &split.errors {
            println!("  error: {error}");
        }
        println!();
    }

    for realm in &report.realms {
        println!(
            "Realm '{}'  backend={}  at {}",
            realm.realm,
            realm.backend.as_deref().unwrap_or("unknown"),
            realm.root.display()
        );
        for entry in &realm.ledger {
            let database = entry
                .database
                .strip_prefix(&realm.root)
                .map(|relative| relative.display().to_string())
                .unwrap_or_else(|_| entry.database.display().to_string());
            println!(
                "  ledger {database} [{}]: {} -> {} ({})",
                entry.domain,
                describe_version(entry.before),
                describe_version(entry.after),
                describe_ledger_action(entry.action)
            );
        }
        if let Some(adoption) = &realm.adoption {
            match &adoption.skipped {
                Some(skipped) => println!(
                    "  adoption: {skipped} (legacy pending: {}, verified: {})",
                    adoption.legacy_pending, adoption.already_verified
                ),
                None => println!(
                    "  adoption: scanned={} already_verified={} adopted={} refused={}",
                    adoption.scanned,
                    adoption.already_verified,
                    adoption.adopted,
                    adoption.refused.len()
                ),
            }
            for refusal in &adoption.refused {
                println!("    refused {}: {}", refusal.session_id, refusal.reason);
            }
        }
        for note in &realm.notes {
            println!("  note: {note}");
        }
        for error in &realm.errors {
            println!("  error: {error}");
        }
        println!();
    }

    if !report.findings.is_empty() {
        println!("Leftovers (report-only):");
        for finding in &report.findings {
            let path = finding
                .path
                .as_ref()
                .map(|path| format!(" at {}", path.display()))
                .unwrap_or_default();
            println!("  [{}] {}{path}", finding.code, finding.message);
        }
        println!();
    }

    let error_count = report.errors.len()
        + report
            .realms
            .iter()
            .map(|realm| realm.errors.len())
            .sum::<usize>();
    for error in &report.errors {
        println!("error: {error}");
    }
    println!("storage migrate: {error_count} error(s)");
}

/// Human-readable prune report.
pub(crate) fn print_prune_report_text(report: &PruneReport) {
    let mode = match report.mode {
        MigrateMode::Apply => "apply",
        _ => "dry-run",
    };
    println!(
        "Storage prune ({mode}, older than {} day(s)) over {} root(s):",
        report.older_than_days,
        report.swept_roots.len()
    );
    for root in &report.swept_roots {
        println!("  {}", root.display());
    }
    println!();
    if report.artifacts.is_empty() {
        println!("No registered maintenance artifacts found.");
    }
    for artifact in &report.artifacts {
        let action = match artifact.action {
            PruneAction::WouldDelete => "would delete",
            PruneAction::Deleted => "deleted",
            PruneAction::Kept => "kept (younger than threshold)",
            PruneAction::DeleteFailed => "DELETE FAILED",
            _ => "unknown",
        };
        println!(
            "  {}  {} bytes, {} day(s) old — {action}",
            artifact.path.display(),
            artifact.bytes,
            artifact.age_days
        );
    }
    for error in &report.errors {
        println!("error: {error}");
    }
    println!("storage prune: {} error(s)", report.errors.len());
}
