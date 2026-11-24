//! A rig, not a row: it writes a memory store that holds a purged record
//! and a removed shared record into a state directory the caller names, so
//! that an earlier release's gateway can be started on that directory and
//! read it. Ignored by default; nothing else runs it.

use super::*;
use crate::identity_first::agent_memory::AgentMemoryProvider;
use crate::memory::SqliteAgentMemoryStore;
use crate::memory::capabilities::StewardStore;
use crate::memory::records::{
    MemoryAuthor, MemoryKind, MemoryScope, NewMemoryRecord, RecordStatus, TrustTier,
};
use crate::memory::staged::{StagedBatchKind, StagedMemoryStore, StagedMutationBatch, StagedOp};

/// The environment variable that names the state directory to write into.
const GUARD_DIR: &str = "MEERKAT_MEMORY_GUARD_DIR";
/// The realm a gateway keeps agent memory in unless it is told another.
const REALM: &str = "default";
/// The agent whose scope holds the purged record.
const IDENTITY: &str = "guard:keeper";
/// The mob whose shared scope holds the removed record.
const MOB: &str = "memory-guard";

fn wording(body: &str) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: "Home address".to_string(),
        description: "Where the family lives".to_string(),
        body: body.to_string(),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    }
}

/// Writes, under `<dir>/agent-memory` (where a gateway with agent memory on
/// keeps its store) and in the realm `default`:
///
/// - in the agent's scope: an active record; and a record that was updated
///   once, forgotten and purged (its earlier version is blank too);
/// - in the mob's shared scope: an active record and a removed one, which
///   keeps its text.
///
/// The ids are written to `<dir>/memory-guard-ids.json` for the reader.
#[tokio::test]
#[ignore = "a rig: writes a store into the directory MEERKAT_MEMORY_GUARD_DIR names"]
async fn write_a_store_with_a_purged_record_and_a_removed_shared_record()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let state_dir = std::path::PathBuf::from(
        std::env::var(GUARD_DIR)
            .map_err(|_| format!("{GUARD_DIR} names the state directory to write into"))?,
    );
    let memory_dir = state_dir.join("agent-memory");
    std::fs::create_dir_all(&memory_dir)?;
    let store = SqliteAgentMemoryStore::open(memory_dir.as_path())?;
    let operator = || MemoryAuthor::Operator { principal: None };
    let own = MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: IDENTITY.to_string(),
    };
    let shared = MemoryScope::Mob {
        realm: REALM.to_string(),
        mob: MOB.to_string(),
    };

    let kept = store
        .remember_authored(&own, wording("the record that stays"), operator())
        .await?
        .memory_id;
    let earlier = store
        .remember_authored(&own, wording("12 Example Lane"), operator())
        .await?
        .memory_id;
    let purged = store
        .supersede_authored(&own, &earlier, wording("14 Example Lane"), operator())
        .await?
        .memory_id;
    store.forget_authored(&own, &purged, operator()).await?;
    let outcome = store
        .purge_authored(&own, &purged, operator())
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert!(outcome.purged, "{outcome:?}");
    assert_eq!(
        outcome.purged_ids,
        vec![purged.clone(), earlier.clone()],
        "{outcome:?}"
    );

    for (id, body) in [
        ("guard-shared-kept", "a shared record that stays"),
        ("guard-shared-removed", "a shared record that was removed"),
    ] {
        let batch = StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: REALM.to_string(),
            author: MemoryAuthor::Application,
            ops: vec![StagedOp::Create {
                id: Some(id.to_string()),
                scope: shared.clone(),
                record: wording(body),
                trust: TrustTier::AgentObserved,
                derived_from: Vec::new(),
                rationale: None,
                created_at_ms: None,
                updated_at_ms: None,
            }],
        };
        let token = store
            .stage(batch)
            .await
            .map_err(|error| format!("{error:?}"))?;
        store
            .commit(token)
            .await
            .map_err(|error| format!("{error:?}"))?;
    }
    let removed = store
        .forget_authored(&shared, "guard-shared-removed", operator())
        .await?;
    assert!(removed.deleted, "the shared record is removed");

    // What the store holds, as this build reads it.
    let read = store
        .records_by_ids(
            REALM,
            &[
                kept.clone(),
                earlier.clone(),
                purged.clone(),
                "guard-shared-kept".to_string(),
                "guard-shared-removed".to_string(),
            ],
        )
        .await?;
    assert_eq!(read.len(), 5, "{read:#?}");
    for record in &read {
        let blank = record.id == purged || record.id == earlier;
        assert_eq!(record.body.is_empty(), blank, "{record:#?}");
    }
    assert!(
        read.iter()
            .any(|record| record.id == purged && record.status == RecordStatus::Tombstoned),
        "{read:#?}"
    );

    std::fs::write(
        state_dir.join("memory-guard-ids.json"),
        serde_json::to_vec_pretty(&json!({
            "realm": REALM,
            "identity": IDENTITY,
            "mob": MOB,
            "kept": kept,
            "purged": purged,
            "earlier_version": earlier,
            "shared_kept": "guard-shared-kept",
            "shared_removed": "guard-shared-removed",
        }))?,
    )?;
    Ok(())
}
