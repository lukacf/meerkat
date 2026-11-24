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

/// The grants through a wildcard, each held by a caller with no other rule.
/// A pattern that covers `mob.memory.delete` (`mob.memory.*`, `mob.*`, `*`)
/// removes a record of the mob's shared memory. A pattern of the agent
/// family (`agent.memory.*`, `agent.*`) covers no mob action and removes
/// none there; with no agent named it covers `agent.memory.delete` and so
/// removes a realm record, which a pattern of the mob family does not.
/// Both removals need their grant with no resource named: a rule that
/// selects a resource, even the agents of every mob, removes neither.
#[tokio::test]
async fn console_forget_in_scope_is_granted_by_each_wildcard_that_covers_its_action() -> RowResult {
    /// (the caller, its one pattern, covers the mob's action, covers the
    /// realm's action)
    const CALLERS: [(&str, &str, bool, bool); 5] = [
        ("dana@example.test", "mob.memory.*", true, false),
        ("erin@example.test", "mob.*", true, false),
        ("frank@example.test", "*", true, true),
        ("gina@example.test", "agent.memory.*", false, true),
        ("hank@example.test", "agent.*", false, true),
    ];
    /// The caller whose one rule holds every action for the agents of
    /// every mob: a rule with a resource selector.
    const SELECTING: &str = "ivan@example.test";
    let fixture = ConsoleMemoryFixture::new("console-forget-in-scope-wildcards").await?;
    for index in 0..=CALLERS.len() {
        fixture
            .seed(
                &format!("mem-shared-{index}"),
                mob_scope(),
                &format!("Shared note {index}"),
                &format!("shared body {index}"),
            )
            .await;
        fixture
            .seed(
                &format!("mem-realm-{index}"),
                realm_scope(),
                &format!("Realm note {index}"),
                &format!("realm body {index}"),
            )
            .await;
    }
    let mut rules: Vec<AccessRule> = CALLERS
        .iter()
        .enumerate()
        .map(|(index, (subject, pattern, _, _))| {
            allow(&format!("wildcard-{index}"), subject, &[*pattern], &[])
        })
        .collect();
    rules.push(AccessRule {
        id: "wildcard-for-the-agents-of-every-mob".to_string(),
        subjects: vec![SELECTING.to_string()],
        actions: vec!["*".to_string()],
        mobs: vec!["*".to_string()],
        ..AccessRule::default()
    });
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules,
        ..AccessControlConfig::default()
    })?;

    // The rule that selects a resource: neither shared scope.
    let selecting = ConsoleCaller::person(&controller, SELECTING);
    let shared_id = format!("mem-shared-{}", CALLERS.len());
    let realm_id = format!("mem-realm-{}", CALLERS.len());
    let mob = fixture
        .rpc_as(&selecting, FORGET_IN_SCOPE, mob_params(MOB, &shared_id))
        .await;
    assert_denied(&mob, crate::access::ACTION_MOB_MEMORY_DELETE);
    let realm = fixture
        .rpc_as(&selecting, FORGET_IN_SCOPE, realm_params(&realm_id))
        .await;
    assert_denied(&realm, crate::access::ACTION_AGENT_MEMORY_DELETE);
    for id in [&shared_id, &realm_id] {
        assert_eq!(
            fixture.record(id).await.status,
            RecordStatus::Active,
            "{id}"
        );
    }

    for (index, (subject, pattern, covers_mob, covers_realm)) in CALLERS.iter().enumerate() {
        let caller = ConsoleCaller::person(&controller, subject);
        let shared_id = format!("mem-shared-{index}");
        let realm_id = format!("mem-realm-{index}");

        let mob = fixture
            .rpc_as(&caller, FORGET_IN_SCOPE, mob_params(MOB, &shared_id))
            .await;
        if *covers_mob {
            assert_eq!(
                mob["result"],
                json!({ "memory_id": shared_id, "deleted": true }),
                "{pattern}: {mob:#?}"
            );
            assert_eq!(
                fixture.record(&shared_id).await.status,
                RecordStatus::Tombstoned,
                "{pattern}"
            );
        } else {
            assert_denied(&mob, crate::access::ACTION_MOB_MEMORY_DELETE);
            assert_eq!(
                fixture.record(&shared_id).await.status,
                RecordStatus::Active,
                "{pattern}"
            );
        }

        let realm = fixture
            .rpc_as(&caller, FORGET_IN_SCOPE, realm_params(&realm_id))
            .await;
        if *covers_realm {
            assert_eq!(
                realm["result"],
                json!({ "memory_id": realm_id, "deleted": true }),
                "{pattern}: {realm:#?}"
            );
            assert_eq!(
                fixture.record(&realm_id).await.status,
                RecordStatus::Tombstoned,
                "{pattern}"
            );
        } else {
            assert_denied(&realm, crate::access::ACTION_AGENT_MEMORY_DELETE);
            assert_eq!(
                fixture.record(&realm_id).await.status,
                RecordStatus::Active,
                "{pattern}"
            );
        }
    }
    Ok(())
}
