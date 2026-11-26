//! The realm kind of `meta/agent_memory/purge` over the stdio RPC surface.

use super::scoped_memory::{REALM, RowResult, ScopedMemoryFixture};
use super::*;
use crate::memory::records::{MemoryScope, RecordStatus};

/// A realm-scoped record is retired by `forget_in_scope` and purged by the
/// same scope form: the kind, with no key.
#[tokio::test]
async fn purge_blanks_a_retired_realm_record() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-purge-realm").await?;
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
    let params = json!({ "realm": REALM, "scope": { "kind": "realm" }, "memory_id": "mem-realm" });
    let removed = fixture
        .rpc("meta/agent_memory/forget_in_scope", params.clone())
        .await;
    assert_eq!(removed["result"]["deleted"], json!(true), "{removed:#?}");

    let response = fixture.rpc("meta/agent_memory/purge", params).await;

    assert_eq!(
        response["result"],
        json!({ "memory_id": "mem-realm", "purged": true, "purged_ids": ["mem-realm"] }),
        "{response:#?}"
    );
    let record = fixture.record("mem-realm").await;
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert_eq!((record.title.as_str(), record.body.as_str()), ("", ""));
    Ok(())
}
