//! Temporary-council store wiring.
//!
//! Councils are a meerkat-mob capability: one `council` tool call seats forked
//! participants, runs bounded exchanges, merges and tears the temporary mob
//! down. `meerkat_mob_mcp::MobMcpState` owns the store and defaults it to an
//! IN-MEMORY implementation, so before this module a Meta gateway lost every
//! council record on restart - including the unfinished ones that
//! `TemporaryCouncilStore::list_unfinished` exists to recover.
//!
//! Meta supplies its own store rather than calling
//! `MobMcpState::with_persistent_storage_root`. That helper would also flip
//! forked-participant custody and realm profiles to durable in the same call:
//! three behaviour changes bundled as one, two of them unrelated to councils
//! and each with its own recovery semantics. Supplying one store keeps the
//! change to the thing it claims to change.
//!
//! The path comes from [`crate::MetaStorageLayout::council_db`], which means
//! the file is a registered [`crate::storage_layout::DatabaseSlot`] and is seen
//! by `storage doctor` and `storage migrate` like every other Meta database.
//! A durable store outside that enumeration would be a second authority.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat_mob::store::{SqliteTemporaryCouncilStore, TemporaryCouncilStore};

/// Canonical file name for the temporary-council store.
pub const COUNCIL_STORE_FILE: &str = "council.sqlite3";

/// Open the durable council store for this layout.
///
/// # Absent is normal; unreadable is not
///
/// These two are deliberately NOT the same outcome, and the first version of
/// this function conflated them.
///
/// A store file that does not exist yet is the ordinary first-boot case. There
/// is nothing to recover, `open` creates it, and a gateway proceeds.
///
/// A store file that EXISTS and cannot be opened is a read/decode failure -
/// corruption, a permission fault, a truncated database. Meerkat's council
/// custody recovery (#1050) surfaces exactly that as a typed refusal, and its
/// owner was explicit: **read/decode failure is not emptiness and must never
/// fail soft**. Degrading to an in-memory store there would present a corrupt
/// durable store as "councils are process-bound today", which is precisely the
/// signal the recovery path exists to raise - swallowed at the boundary before
/// it ever reaches them.
///
/// So this returns `Err` when the file is present and unopenable, and the
/// caller refuses the boot. It returns `Ok(None)` only when councils are
/// legitimately unavailable without evidence of damage.
pub fn open_council_store(
    path: &Path,
) -> Result<Option<Arc<dyn TemporaryCouncilStore>>, CouncilStoreError> {
    match SqliteTemporaryCouncilStore::open(path) {
        Ok(store) => Ok(Some(Arc::new(store))),
        Err(error) => {
            // `path.exists()` is the discriminator. It is a race in principle -
            // the file could appear between the failed open and this check -
            // but the two outcomes of losing that race are "refuse a boot that
            // would have worked" and "degrade on a store that was fine for one
            // instant", and refusing is the safe side of a custody question.
            if path.exists() {
                Err(CouncilStoreError::Unreadable {
                    path: path.display().to_string(),
                    detail: error.to_string(),
                })
            } else {
                tracing::warn!(
                    error = %error,
                    path = %path.display(),
                    "council store absent and could not be created; councils fall back \
                     to process-bound in-memory storage and will not survive a restart",
                );
                Ok(None)
            }
        }
    }
}

/// A durable council store that exists on disk and cannot be read.
///
/// Typed rather than a bare string so a caller can refuse on it without
/// pattern-matching a log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CouncilStoreError {
    /// The store file is present and could not be opened.
    Unreadable {
        /// Where the unreadable store lives.
        path: String,
        /// Underlying detail, already bounded by the store layer.
        detail: String,
    },
}

impl std::fmt::Display for CouncilStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { path, detail } => write!(
                f,
                "durable council store at {path} exists but could not be read: {detail}. \
                 Refusing rather than degrading to in-memory storage, because an \
                 unreadable custody store is not an empty one."
            ),
        }
    }
}

impl std::error::Error for CouncilStoreError {}

/// Resolve the council store path for a `store_path`-style root.
///
/// `MetaStorageLayout::standalone_from_store_path` is the ONE place that
/// interprets the `store_path` escape hatch (an existing directory IS the state
/// dir; otherwise a value with an extension names a database and its parent is
/// the state dir; otherwise it IS the state dir).
/// Re-deriving that rule here would create a second path authority, so this
/// goes through the layout even though it only reads `state_dir`.
///
/// `gateway_home` is immaterial to `council_db()`, which reads `state_dir`
/// alone; callers that need a real gateway home must build the layout properly
/// rather than reusing this helper.
#[must_use]
pub fn council_db_for_store_path(store_path: &Path) -> PathBuf {
    crate::MetaStorageLayout::standalone_from_store_path(store_path, store_path.to_path_buf())
        .council_db()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use meerkat_mob::machines::temporary_council_lifecycle::{
        TemporaryCouncilLifecycleInput, TemporaryCouncilLifecycleMachineAuthority,
        TemporaryCouncilLifecycleMachineMutator,
    };
    use meerkat_mob::store::{
        SqliteTemporaryCouncilStore, TemporaryCouncilRecord, TemporaryCouncilStore,
    };
    use meerkat_mob::temporary_council::{
        TemporaryCouncilDurability, TemporaryCouncilExitReason, TemporaryCouncilId,
    };

    /// The shape a crashed coordinator leaves: a council opened and claimed by
    /// the previous process, no result, and a claim lease that has already
    /// lapsed (so the first recovery pass may take it over).
    async fn write_interrupted_council(
        store: &dyn TemporaryCouncilStore,
        council_id: &TemporaryCouncilId,
    ) {
        let fingerprint = format!("tcf1:sha256:{council_id}");
        let mut authority = TemporaryCouncilLifecycleMachineAuthority::new();
        TemporaryCouncilLifecycleMachineMutator::apply(
            &mut authority,
            TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: fingerprint.clone(),
            },
        )
        .expect("open the council record");
        TemporaryCouncilLifecycleMachineMutator::apply(
            &mut authority,
            TemporaryCouncilLifecycleInput::Claim {
                claim_id: "coordinator-of-the-dead-process".to_string(),
                lease_expired: false,
            },
        )
        .expect("the previous process's coordinator claims it");
        let created = chrono::Utc::now() - chrono::Duration::seconds(600);
        store
            .insert_new(&TemporaryCouncilRecord {
                council_id: council_id.clone(),
                request_fingerprint: fingerprint,
                temporary_mob_id: council_id.temporary_mob_id(),
                deadline: created + chrono::Duration::seconds(1200),
                machine_state: authority.state().clone(),
                durability: TemporaryCouncilDurability::Durable,
                claim_lease_expires_at: created + chrono::Duration::seconds(120),
                participants: Vec::new(),
                exchanges: Vec::new(),
                result: None,
                cleanup: None,
                detached_job: None,
                revision: 0,
                created_at: created,
                updated_at: created,
            })
            .await
            .expect("write the interrupted council record");
    }

    /// Meta's persistent composition supplies a durable council store and
    /// restores its mob through `mob_insert_handle`. With the state shared via
    /// `into_shared`, that restore runs meerkat's council sweep: a council the
    /// previous process left mid-run is sealed as coordinator-interrupted.
    #[tokio::test(flavor = "multi_thread")]
    async fn persistent_restore_recovers_councils_a_restart_interrupted() {
        let temp = tempfile::tempdir().expect("tempdir");
        let store_path = temp.path().join("state");
        std::fs::create_dir_all(&store_path).expect("state dir");
        let council_db = super::council_db_for_store_path(&store_path);
        let council_id = TemporaryCouncilId::new("interrupted-by-restart").expect("council id");
        {
            let previous = SqliteTemporaryCouncilStore::open(&council_db).expect("council store");
            write_interrupted_council(&previous, &council_id).await;
        }

        let definition = meerkat_mob::MobDefinition::from_toml(
            "[mob]\nid = \"council-restore\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n",
        )
        .expect("mob definition");
        let spec = crate::MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            store_path,
            4,
        )
        .await
        .expect("persistent spec")
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = crate::mob_handle_runtime::MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap the restored runtime");

        let probe = SqliteTemporaryCouncilStore::open(&council_db).expect("probe council store");
        let record = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let record = probe
                    .load(&council_id)
                    .await
                    .expect("load council")
                    .expect("council record");
                if record.result.is_some() {
                    return record;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the restored runtime never recovered the interrupted council");
        assert_eq!(
            record.result.expect("sealed result").exit_reason,
            TemporaryCouncilExitReason::CoordinatorInterrupted
        );
        let _ = runtime.handle().shutdown().await;
    }
}
