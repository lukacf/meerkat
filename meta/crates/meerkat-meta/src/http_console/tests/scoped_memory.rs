//! Rows for `meta/agent_memory/forget_in_scope` on the Console.

use super::memory_fixture::{ConsoleMemoryFixture, MOB, REALM, RowResult, mob_scope};
use super::*;
use crate::memory::records::{MemoryScope, RecordStatus};

fn mob_params(memory_id: &str) -> Value {
    json!({ "realm": REALM, "scope": { "kind": "mob", "key": MOB }, "memory_id": memory_id })
}

/// A removal from the mob's shared memory has an action of its own. The
/// requirement holds for a malformed request too, so a caller without the
/// action learns nothing from the parse.
#[test]
fn forget_in_scope_requires_mob_memory_delete() {
    for params in [
        mob_params("mem-shared"),
        json!({ "scope": { "kind": "identity", "key": "lead:main" }, "memory_id": "mem-shared" }),
        json!({ "memory_id": "mem-shared" }),
    ] {
        assert_eq!(
            super::super::console_rpc_access_requirements(
                "meta/agent_memory/forget_in_scope",
                &params
            ),
            Some(vec![(crate::access::ACTION_MOB_MEMORY_DELETE, None)]),
            "{params}"
        );
    }
    assert!(super::super::is_console_mutating_rpc_method(
        "meta/agent_memory/forget_in_scope"
    ));
    assert!(
        crate::access::ACCESS_ACTIONS.contains(&crate::access::ACTION_MOB_MEMORY_DELETE),
        "the action is part of the vocabulary, so a rule may name it"
    );
}

/// The realm's own scope rides the unscoped delete grant, as its reads ride
/// the unscoped read grant.
#[test]
fn forget_in_scope_of_a_realm_record_requires_the_unscoped_delete_grant() {
    assert_eq!(
        super::super::console_rpc_access_requirements(
            "meta/agent_memory/forget_in_scope",
            &json!({ "realm": REALM, "scope": { "kind": "realm" }, "memory_id": "mem-realm" }),
        ),
        Some(vec![(crate::access::ACTION_AGENT_MEMORY_DELETE, None)])
    );
}

/// On the Console the removal is the operator's, and a caller who may
/// change nothing there removes nothing.
#[tokio::test]
async fn console_forget_in_scope_tombstones_a_mob_record_as_the_operator() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope").await?;
    fixture
        .seed("mem-shared", mob_scope(), "Shared note", "shared body")
        .await;

    let read_only = fixture
        .rpc(
            "meta/agent_memory/forget_in_scope",
            mob_params("mem-shared"),
            false,
        )
        .await;
    assert!(!read_only["error"].is_null(), "{read_only:#?}");
    assert_eq!(
        fixture.record("mem-shared").await.status,
        RecordStatus::Active
    );

    let response = fixture
        .rpc(
            "meta/agent_memory/forget_in_scope",
            mob_params("mem-shared"),
            true,
        )
        .await;
    assert!(response["error"].is_null(), "{response:#?}");
    assert_eq!(
        response["result"],
        json!({ "memory_id": "mem-shared", "deleted": true })
    );
    assert_eq!(
        fixture.record("mem-shared").await.status,
        RecordStatus::Tombstoned
    );
    assert_eq!(
        fixture.audit_authors("mem-shared", "tombstone"),
        vec![json!({ "author": "operator" })],
        "an open Console knows no principal; the author is still the operator"
    );
    Ok(())
}

#[tokio::test]
async fn console_forget_in_scope_removes_a_realm_record() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope-realm").await?;
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
            true,
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

#[tokio::test]
async fn console_advertises_forget_in_scope_only_where_it_may_change_something() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope-advertised").await?;
    let method = "meta/agent_memory/forget_in_scope".to_string();
    assert!(fixture.advertised(true).await.contains(&method));
    assert!(!fixture.advertised(false).await.contains(&method));
    Ok(())
}

/// On the Console too, the removal confirms the id of a record kept
/// under a member's owner in no shared scope, and the record stays.
#[tokio::test]
async fn console_forget_in_scope_reaches_no_record_kept_under_a_members_owner() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope-owner-kept").await?;
    let kept = MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: crate::memory::records::private_member_scope_key(
            "console-caller:v1:alice",
            "lead:main",
        ),
    };
    fixture
        .seed("mem-kept", kept.clone(), "Own note", "own body")
        .await;
    for params in [
        mob_params("mem-kept"),
        json!({ "realm": REALM, "scope": { "kind": "realm" }, "memory_id": "mem-kept" }),
    ] {
        let response = fixture
            .rpc("meta/agent_memory/forget_in_scope", params.clone(), true)
            .await;
        assert_eq!(
            response["result"],
            json!({ "memory_id": "mem-kept", "deleted": false }),
            "{params}: {response:#?}"
        );
    }
    let record = fixture.record("mem-kept").await;
    assert_eq!(record.status, RecordStatus::Active);
    assert_eq!(record.scope, kept);
    Ok(())
}
