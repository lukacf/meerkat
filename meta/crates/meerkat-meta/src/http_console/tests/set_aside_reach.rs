//! Rows that drive the purge and the forget on the Console through a
//! caller's access view on a note that was set aside: an access
//! administrator reaches it under the name it was set aside from, and
//! nobody else does.

use super::memory_fixture::{ConsoleCaller, ConsoleMemoryFixture, REALM, RowResult, record_member};
use super::*;
use crate::access::{AccessControlConfig, AccessRule};
use crate::memory::records::{MemoryScope, RecordStatus};

const ADMIN: &str = "admin@example.test";
const ALICE: &str = "alice@example.test";
const ALICE_KEY: &str = "console-caller:v1:alice";
const BOB: &str = "bob@example.test";

/// A plain name whose notes were set aside, and that a member every caller
/// may know holds now.
const NAME: &str = "identity:reused";

const PURGE: &str = "meta/agent_memory/purge";
const FORGET: &str = "meta/agent_memory/forget";

/// The scope the notes of `name` were set aside under, by the key the
/// setting aside itself writes.
fn set_aside(name: &str) -> MemoryScope {
    MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: crate::memory::sqlite_store::set_aside_scope_key(name),
    }
}

fn named(memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": NAME, "memory_id": memory_id })
}

/// The access configurations a Console that can identify a person runs
/// with: rules on, where Alice and Bob hold the purge and the delete grant
/// on every agent, and rules off.
fn configurations() -> [AccessControlConfig; 2] {
    [
        AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![AccessRule {
                id: "alice-and-bob-forget-and-purge-any-agent".to_string(),
                subjects: vec![ALICE.to_string(), BOB.to_string()],
                actions: vec![
                    crate::access::ACTION_AGENT_MEMORY_ADMIN.to_string(),
                    crate::access::ACTION_AGENT_MEMORY_DELETE.to_string(),
                ],
                agents: vec!["*".to_string()],
                ..AccessRule::default()
            }],
            ..AccessControlConfig::default()
        },
        AccessControlConfig {
            enabled: false,
            admins: vec![ADMIN.to_string()],
            ..AccessControlConfig::default()
        },
    ]
}

/// A set-aside note is purged on the Console by an access administrator,
/// under the name it was set aside from. Anybody else who names that name
/// and the note's id, with the purge grant on every agent or with the
/// rules off, and whether or not the notes were theirs, is answered as for
/// a record that is not there, and nothing is blanked.
#[tokio::test]
async fn console_purge_reaches_a_set_aside_note_for_an_administrator_alone() -> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-purge-set-aside-{index}")).await?;
        fixture
            .seed_retired(
                "gone-aside",
                set_aside(NAME),
                "Home address",
                "12 Example Lane",
            )
            .await;
        let controller = AccessController::new(config)?;
        record_member(&controller, NAME, None);
        let bob = ConsoleCaller::person(&controller, BOB);
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        for (caller, who) in [(&bob, "another person"), (&alice, "a person with a key")] {
            let response = fixture.rpc_as(caller, PURGE, named("gone-aside")).await;
            assert!(
                response["error"].is_null(),
                "rules on {enabled}, {who}: {response:#?}"
            );
            assert_eq!(
                response["result"],
                json!({ "memory_id": "gone-aside", "purged": false, "purged_ids": [] }),
                "rules on {enabled}, {who}: {response:#?}"
            );
            assert_eq!(
                fixture.record("gone-aside").await.body,
                "12 Example Lane",
                "rules on {enabled}, {who}"
            );
        }

        let administrators = fixture.rpc_as(&admin, PURGE, named("gone-aside")).await;
        assert_eq!(
            administrators["result"],
            json!({ "memory_id": "gone-aside", "purged": true, "purged_ids": ["gone-aside"] }),
            "rules on {enabled}: {administrators:#?}"
        );
        let blanked = fixture.record("gone-aside").await;
        assert_eq!(blanked.body, "", "rules on {enabled}");
        assert_eq!(blanked.scope, set_aside(NAME), "the row stays");
    }
    Ok(())
}

/// The Console's forget reaches a record of the set-aside scope for an
/// access administrator alone: anybody else is answered exactly as for an
/// id no record holds, and the record stays.
#[tokio::test]
async fn console_forget_reaches_a_set_aside_record_for_an_administrator_alone() -> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-forget-set-aside-{index}")).await?;
        fixture
            .seed(
                "live-aside",
                set_aside(NAME),
                "Home address",
                "12 Example Lane",
            )
            .await;
        let controller = AccessController::new(config)?;
        record_member(&controller, NAME, None);
        let bob = ConsoleCaller::person(&controller, BOB);
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        for (caller, who) in [(&bob, "another person"), (&alice, "a person with a key")] {
            let no_such_record = fixture.rpc_as(caller, FORGET, named("live-nowhere")).await;
            let response = fixture.rpc_as(caller, FORGET, named("live-aside")).await;
            assert_eq!(
                response.to_string().replace("live-aside", "live-nowhere"),
                no_such_record.to_string(),
                "rules on {enabled}, {who}: the id opens nothing"
            );
            assert_eq!(
                fixture.record("live-aside").await.status,
                RecordStatus::Active,
                "rules on {enabled}, {who}"
            );
        }

        let administrators = fixture.rpc_as(&admin, FORGET, named("live-aside")).await;
        assert_eq!(
            administrators["result"],
            json!({ "memory_id": "live-aside", "deleted": true }),
            "rules on {enabled}: {administrators:#?}"
        );
        let retired = fixture.record("live-aside").await;
        assert_eq!(retired.status, RecordStatus::Tombstoned);
        assert_eq!(retired.scope, set_aside(NAME), "the row stays");
    }
    Ok(())
}
