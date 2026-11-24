//! The realm kind of `meta/agent_memory/forget_in_scope` over the stdio RPC
//! surface.

use super::scoped_memory::{REALM, RowResult, ScopedMemoryFixture};
use super::*;
use crate::memory::records::{MemoryScope, RecordStatus};

/// No stock path creates a realm-scoped record; a host that writes one in
/// Rust can retire it by this call. The scope has no key.
#[tokio::test]
async fn forget_in_scope_tombstones_a_realm_record() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-in-scope-realm").await?;
    fixture
        .seed(
            "mem-realm",
            MemoryScope::Realm {
                realm: REALM.to_string(),
            },
            "Realm note",
            "realm body",
        )
        .await;

    let response = fixture
        .rpc(
            "meta/agent_memory/forget_in_scope",
            json!({ "realm": REALM, "scope": { "kind": "realm" }, "memory_id": "mem-realm" }),
        )
        .await;

    assert_eq!(
        response["result"],
        json!({ "memory_id": "mem-realm", "deleted": true }),
        "{response:#?}"
    );
    assert_eq!(
        fixture.record("mem-realm").await.status,
        RecordStatus::Tombstoned
    );
    Ok(())
}
