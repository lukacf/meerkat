//! Rows that drive `meta/agent_memory/forget_in_scope` on the Console
//! through a caller's access view: its grants end to end.

use super::memory_fixture::{
    ConsoleCaller, ConsoleMemoryFixture, MOB, REALM, RowResult, mob_scope,
};
use super::*;
use crate::access::{AccessControlConfig, AccessRule};
use crate::memory::records::{MemoryScope, RecordStatus};

const ADMIN: &str = "admin@example.test";
const ALICE: &str = "alice@example.test";
const BOB: &str = "bob@example.test";
const CAROL: &str = "carol@example.test";

const FORGET_IN_SCOPE: &str = "meta/agent_memory/forget_in_scope";

fn realm_scope() -> MemoryScope {
    MemoryScope::Realm {
        realm: REALM.to_string(),
    }
}

fn mob_params(mob: &str, memory_id: &str) -> Value {
    json!({ "realm": REALM, "scope": { "kind": "mob", "key": mob }, "memory_id": memory_id })
}

fn realm_params(memory_id: &str) -> Value {
    json!({ "realm": REALM, "scope": { "kind": "realm" }, "memory_id": memory_id })
}

fn allow(id: &str, subject: &str, actions: &[&str], agents: &[&str]) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: actions.iter().map(ToString::to_string).collect(),
        agents: agents.iter().map(ToString::to_string).collect(),
        ..AccessRule::default()
    }
}

fn assert_denied(response: &Value, action: &str) {
    assert_eq!(
        response["error"]["data"]["kind"],
        json!("access_denied"),
        "{response:#?}"
    );
    assert_eq!(
        response["error"]["data"]["action"],
        json!(action),
        "{response:#?}"
    );
}

/// The grants end to end. A removal from a mob's shared memory needs
/// `mob.memory.delete`; a removal from the realm's needs the delete grant
/// with no agent named. Neither stands for the other, and the delete grant
/// on one agent stands for neither.
#[tokio::test]
async fn console_forget_in_scope_enforces_its_grant_per_scope_kind() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope-grants").await?;
    fixture
        .seed("mem-shared", mob_scope(), "Shared note", "shared body")
        .await;
    fixture
        .seed("mem-realm", realm_scope(), "Realm note", "realm body")
        .await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            allow(
                "bob-removes-mob-records",
                BOB,
                &[crate::access::ACTION_MOB_MEMORY_DELETE],
                &[],
            ),
            allow(
                "alice-forgets-anywhere",
                ALICE,
                &[crate::access::ACTION_AGENT_MEMORY_DELETE],
                &[],
            ),
            allow(
                "carol-forgets-for-one-agent",
                CAROL,
                &[crate::access::ACTION_AGENT_MEMORY_DELETE],
                &["identity:shared"],
            ),
        ],
        ..AccessControlConfig::default()
    })?;
    let bob = ConsoleCaller::person(&controller, BOB);
    let alice = ConsoleCaller::person(&controller, ALICE);
    let carol = ConsoleCaller::person(&controller, CAROL);

    // The delete grant on one agent: neither shared scope.
    let mob = fixture
        .rpc_as(&carol, FORGET_IN_SCOPE, mob_params(MOB, "mem-shared"))
        .await;
    assert_denied(&mob, crate::access::ACTION_MOB_MEMORY_DELETE);
    let realm = fixture
        .rpc_as(&carol, FORGET_IN_SCOPE, realm_params("mem-realm"))
        .await;
    assert_denied(&realm, crate::access::ACTION_AGENT_MEMORY_DELETE);
    // The unscoped delete grant without `mob.memory.delete`: not the mob's.
    let mob = fixture
        .rpc_as(&alice, FORGET_IN_SCOPE, mob_params(MOB, "mem-shared"))
        .await;
    assert_denied(&mob, crate::access::ACTION_MOB_MEMORY_DELETE);
    // `mob.memory.delete` without the unscoped delete grant: not the realm's.
    let realm = fixture
        .rpc_as(&bob, FORGET_IN_SCOPE, realm_params("mem-realm"))
        .await;
    assert_denied(&realm, crate::access::ACTION_AGENT_MEMORY_DELETE);
    for id in ["mem-shared", "mem-realm"] {
        assert_eq!(
            fixture.record(id).await.status,
            RecordStatus::Active,
            "{id}"
        );
    }

    // Each grant removes in its own scope kind.
    let mob = fixture
        .rpc_as(&bob, FORGET_IN_SCOPE, mob_params(MOB, "mem-shared"))
        .await;
    assert_eq!(
        mob["result"],
        json!({ "memory_id": "mem-shared", "deleted": true }),
        "{mob:#?}"
    );
    let realm = fixture
        .rpc_as(&alice, FORGET_IN_SCOPE, realm_params("mem-realm"))
        .await;
    assert_eq!(
        realm["result"],
        json!({ "memory_id": "mem-realm", "deleted": true }),
        "{realm:#?}"
    );
    assert_eq!(
        fixture.audit_authors("mem-shared", "tombstone"),
        vec![json!({ "author": "operator", "principal": BOB })]
    );
    Ok(())
}

/// A record is removed only under the scope it is in: the same id named
/// under another mob's key answers as an id that is not there, and the
/// record stays.
#[tokio::test]
async fn console_forget_in_scope_answers_false_for_another_mobs_key() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope-other-mob").await?;
    fixture
        .seed("mem-shared", mob_scope(), "Shared note", "shared body")
        .await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![allow(
            "bob-removes-mob-records",
            BOB,
            &[crate::access::ACTION_MOB_MEMORY_DELETE],
            &[],
        )],
        ..AccessControlConfig::default()
    })?;
    let bob = ConsoleCaller::person(&controller, BOB);

    let other = fixture
        .rpc_as(&bob, FORGET_IN_SCOPE, mob_params("mob:other", "mem-shared"))
        .await;
    assert_eq!(
        other["result"],
        json!({ "memory_id": "mem-shared", "deleted": false }),
        "{other:#?}"
    );
    let unknown = fixture
        .rpc_as(&bob, FORGET_IN_SCOPE, mob_params(MOB, "mem-nowhere"))
        .await;
    assert_eq!(
        unknown["result"],
        json!({ "memory_id": "mem-nowhere", "deleted": false }),
        "{unknown:#?}"
    );
    assert_eq!(
        fixture.record("mem-shared").await.status,
        RecordStatus::Active
    );
    Ok(())
}
