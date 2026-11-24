//! Rows for the purge and the forget over the stdio RPC surface on a note
//! that was set aside: it lies in the scope its name's notes were set aside
//! under, and the host reaches it under that name.

use super::scoped_memory::{REALM, RowResult, ScopedMemoryFixture, identity_scope};
use super::*;
use crate::memory::records::{MemoryAuthor, MemoryScope, RecordStatus};

/// The identity the fixture registers.
const REGISTERED: &str = "identity:luka";
/// A plain name whose notes were set aside.
const OLD: &str = "identity:old-side";

/// The scope the notes of `name` were set aside under, by the key the
/// setting aside itself writes.
fn set_aside(name: &str) -> MemoryScope {
    identity_scope(&crate::memory::sqlite_store::set_aside_scope_key(name))
}

fn named(identity: &str, memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": identity, "memory_id": memory_id })
}

/// A set-aside note is forgotten where it lies. The host purges it under
/// the name it was set aside from; another name does not reach it. The
/// row stays in the set-aside scope.
#[tokio::test]
async fn the_host_purges_a_set_aside_note_under_the_name_it_was_set_aside_from() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-purge-set-aside").await?;
    fixture
        .seed(
            "mem-aside",
            set_aside(OLD),
            "Home address",
            "12 Example Lane",
        )
        .await;
    let removed = fixture
        .store
        .forget_authored(&set_aside(OLD), "mem-aside", MemoryAuthor::Application)
        .await
        .expect("forget");
    assert!(
        removed.deleted,
        "the note is forgotten, as a set-aside note is"
    );

    let others = fixture
        .rpc("meta/agent_memory/purge", named(REGISTERED, "mem-aside"))
        .await;
    assert_eq!(
        others["result"],
        json!({ "memory_id": "mem-aside", "purged": false, "purged_ids": [] }),
        "{others:#?}"
    );
    assert_eq!(fixture.record("mem-aside").await.body, "12 Example Lane");

    let response = fixture
        .rpc("meta/agent_memory/purge", named(OLD, "mem-aside"))
        .await;
    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(
        response["result"],
        json!({ "memory_id": "mem-aside", "purged": true, "purged_ids": ["mem-aside"] }),
        "{response:#?}"
    );
    let after = fixture.record("mem-aside").await;
    assert_eq!(after.status, RecordStatus::Tombstoned);
    assert_eq!(
        (after.title.as_str(), after.body.as_str()),
        ("", ""),
        "{after:#?}"
    );
    assert_eq!(after.scope, set_aside(OLD), "the row stays where it was");
    Ok(())
}

/// The host's forget reaches a record of the set-aside scope under the
/// name too: one that is not retired yet is retired there, and a note that
/// is retired already, as a set-aside note is, answers `deleted: false`.
/// Another name does not reach it.
#[tokio::test]
async fn the_host_retires_a_set_aside_record_under_the_name_it_was_set_aside_from() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-set-aside").await?;
    fixture
        .seed(
            "mem-aside-live",
            set_aside(OLD),
            "Home address",
            "12 Example Lane",
        )
        .await;
    let forget = |identity: &'static str| {
        fixture.rpc(
            "meta/agent_memory/forget",
            named(identity, "mem-aside-live"),
        )
    };
    let deleted = |deleted: bool| json!({ "memory_id": "mem-aside-live", "deleted": deleted });

    let others = forget(REGISTERED).await;
    assert_eq!(others["result"], deleted(false), "{others:#?}");
    assert_eq!(
        fixture.record("mem-aside-live").await.status,
        RecordStatus::Active
    );

    let first = forget(OLD).await;
    assert!(first["error"].is_null(), "{first:#?}");
    assert_eq!(first["result"], deleted(true), "{first:#?}");
    let record = fixture.record("mem-aside-live").await;
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert_eq!(record.scope, set_aside(OLD), "the row stays where it was");
    assert_eq!(record.body, "12 Example Lane", "retiring does not erase");

    let second = forget(OLD).await;
    assert_eq!(second["result"], deleted(false), "{second:#?}");
    Ok(())
}
