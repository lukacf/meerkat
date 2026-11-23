//! Rows for `meta/agent_memory/forget` over the stdio RPC surface on a
//! record kept under a member's owner: the host retires it under the
//! member's name, whether or not an identity of that name is registered.

use super::scoped_memory::{REALM, RowResult, ScopedMemoryFixture, identity_scope};
use super::*;
use crate::memory::records::{MemoryScope, RecordStatus};

/// The identity the fixture registers.
const REGISTERED: &str = "identity:luka";
/// A member that was private to a person and has ended: no identity of the
/// name is registered.
const ENDED: &str = "identity:ended-side";
/// The person the ended member was private to.
const OWNER: &str = "console-caller:v1:alice";

/// The scope that keeps the memory of the member `member` private to the
/// owner: under the owner and the member's name.
fn kept_scope(member: &str) -> MemoryScope {
    identity_scope(&crate::memory::records::private_member_scope_key(
        OWNER, member,
    ))
}

fn named(identity: &str, memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": identity, "memory_id": memory_id })
}

async fn forget(fixture: &ScopedMemoryFixture, identity: &str, memory_id: &str) -> Value {
    fixture
        .rpc("meta/agent_memory/forget", named(identity, memory_id))
        .await
}

fn deleted(memory_id: &str, deleted: bool) -> Value {
    json!({ "memory_id": memory_id, "deleted": deleted })
}

/// The host is the operator. An active note of a member private to a
/// person lies under that person and the member's name; the host retires
/// it under the member's name, also after the member has ended. The row
/// stays where it was, with its text: retiring does not erase. A second
/// call retires nothing.
#[tokio::test]
async fn forget_retires_an_active_record_kept_under_a_members_owner() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-owner-kept").await?;
    fixture
        .seed(
            "mem-kept",
            kept_scope(ENDED),
            "Home address",
            "12 Example Lane",
        )
        .await;

    let response = forget(&fixture, ENDED, "mem-kept").await;

    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(
        response["result"],
        deleted("mem-kept", true),
        "{response:#?}"
    );
    let record = fixture.record("mem-kept").await;
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert_eq!(
        record.scope,
        kept_scope(ENDED),
        "the row stays where it was"
    );
    assert_eq!(record.body, "12 Example Lane", "retiring does not erase");

    let again = forget(&fixture, ENDED, "mem-kept").await;
    assert_eq!(again["result"], deleted("mem-kept", false), "{again:#?}");
    Ok(())
}

/// Another member's name does not reach a record kept under an owner: a
/// registered identity's name answers as for a record of another scope,
/// and a name no identity holds is refused as before. The record stays
/// active.
#[tokio::test]
async fn forget_under_another_name_reaches_no_record_kept_under_a_members_owner() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-owner-kept-other-name").await?;
    fixture
        .seed(
            "mem-kept",
            kept_scope(ENDED),
            "Home address",
            "12 Example Lane",
        )
        .await;

    let registered = forget(&fixture, REGISTERED, "mem-kept").await;
    assert_eq!(
        registered["result"],
        deleted("mem-kept", false),
        "{registered:#?}"
    );

    let nobody = forget(&fixture, "identity:nobody", "mem-kept").await;
    let absent = forget(&fixture, "identity:nobody", "mem-nowhere").await;
    assert!(nobody["result"].is_null(), "{nobody:#?}");
    assert_eq!(nobody["error"]["code"], json!(-32602), "{nobody:#?}");
    assert_eq!(
        nobody["error"], absent["error"],
        "the record's id changes nothing in the answer for a name no identity holds"
    );

    let record = fixture.record("mem-kept").await;
    assert_eq!(record.status, RecordStatus::Active);
    assert_eq!(record.scope, kept_scope(ENDED));
    Ok(())
}

/// End to end, for the note of a private member that has ended: it is
/// active, so a purge refuses it; the host retires it under the member's
/// name; the host then purges it, and its text is gone while its row
/// stays in its scope.
#[tokio::test]
async fn an_ended_private_members_note_is_retired_and_then_purged_by_the_host() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-then-purge-owner-kept").await?;
    fixture
        .seed(
            "mem-kept",
            kept_scope(ENDED),
            "Home address",
            "12 Example Lane",
        )
        .await;

    let early = fixture
        .rpc("meta/agent_memory/purge", named(ENDED, "mem-kept"))
        .await;
    assert_eq!(
        early["error"]["data"]["kind"],
        json!(crate::identity_first::AgentMemoryPurgeError::NOT_TOMBSTONED_KIND),
        "{early:#?}"
    );

    let retired = forget(&fixture, ENDED, "mem-kept").await;
    assert_eq!(retired["result"], deleted("mem-kept", true), "{retired:#?}");

    let purged = fixture
        .rpc("meta/agent_memory/purge", named(ENDED, "mem-kept"))
        .await;
    assert_eq!(
        purged["result"],
        json!({ "memory_id": "mem-kept", "purged": true, "purged_ids": ["mem-kept"] }),
        "{purged:#?}"
    );
    let record = fixture.record("mem-kept").await;
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert_eq!(
        (record.title.as_str(), record.body.as_str()),
        ("", ""),
        "{record:#?}"
    );
    assert_eq!(record.scope, kept_scope(ENDED));
    Ok(())
}

/// A record under a plain name is retired as before: a registered
/// identity's own record by its name, once; an unknown id and a second
/// call answer `deleted: false`; a name no identity holds is refused, and
/// a record under that plain name stays.
#[tokio::test]
async fn forget_retires_a_plain_name_record_as_before() -> RowResult {
    let fixture = ScopedMemoryFixture::new("rpc-forget-plain-name").await?;
    fixture
        .seed(
            "mem-own",
            identity_scope(REGISTERED),
            "Own note",
            "own body",
        )
        .await;
    fixture
        .seed(
            "mem-orphan",
            identity_scope("identity:nobody"),
            "Orphan note",
            "orphan body",
        )
        .await;

    let first = forget(&fixture, REGISTERED, "mem-own").await;
    assert_eq!(first["result"], deleted("mem-own", true), "{first:#?}");
    let second = forget(&fixture, REGISTERED, "mem-own").await;
    assert_eq!(second["result"], deleted("mem-own", false), "{second:#?}");
    let unknown = forget(&fixture, REGISTERED, "mem-nowhere").await;
    assert_eq!(
        unknown["result"],
        deleted("mem-nowhere", false),
        "{unknown:#?}"
    );
    assert_eq!(
        fixture.record("mem-own").await.status,
        RecordStatus::Tombstoned
    );

    let orphan = forget(&fixture, "identity:nobody", "mem-orphan").await;
    assert!(orphan["result"].is_null(), "{orphan:#?}");
    assert_eq!(orphan["error"]["code"], json!(-32602), "{orphan:#?}");
    assert_eq!(
        fixture.record("mem-orphan").await.status,
        RecordStatus::Active
    );
    Ok(())
}
