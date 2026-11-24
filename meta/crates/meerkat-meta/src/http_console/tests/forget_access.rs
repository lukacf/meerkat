//! Rows that drive `meta/agent_memory/forget` on the Console through a
//! caller's access view, on a record kept under a member's owner: who
//! retires it, and that everybody else is answered as for a member that
//! holds no such record.

use super::memory_fixture::{ConsoleCaller, ConsoleMemoryFixture, REALM, RowResult, record_member};
use super::*;
use crate::access::{AccessControlConfig, AccessRule};
use crate::memory::records::{MemoryScope, RecordStatus};

const ADMIN: &str = "admin@example.test";
const ALICE: &str = "alice@example.test";
const ALICE_KEY: &str = "console-caller:v1:alice";
const BOB: &str = "bob@example.test";

/// A member private to Alice that the Console knows.
const PRIVATE: &str = "identity:private";
/// A member private to Alice that ended; the Console holds nothing about
/// it, and the store keeps its records under her and its name.
const ENDED_PRIVATE: &str = "identity:ended-private";
/// A name a member private to Alice held, and a member every caller may
/// know holds now.
const REUSED: &str = "identity:reused";
/// A name no member ever held.
const NOBODY: &str = "identity:nobody";

const FORGET: &str = "meta/agent_memory/forget";

/// The scope that keeps the memory of the member `member` private to
/// Alice: under her and the member's name.
fn alices_scope(member: &str) -> MemoryScope {
    MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: crate::memory::records::private_member_scope_key(ALICE_KEY, member),
    }
}

fn named(identity: &str, memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": identity, "memory_id": memory_id })
}

fn deleted(memory_id: &str) -> Value {
    json!({ "memory_id": memory_id, "deleted": true })
}

/// The access configurations a Console that can identify a person runs
/// with: rules on, where Alice and Bob hold the delete grant on every
/// agent, and rules off.
fn configurations() -> [AccessControlConfig; 2] {
    [
        AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![AccessRule {
                id: "alice-and-bob-forget-any-agent".to_string(),
                subjects: vec![ALICE.to_string(), BOB.to_string()],
                actions: vec![crate::access::ACTION_AGENT_MEMORY_DELETE.to_string()],
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

/// The whole error of `response`, with `from` written as `to`: the answer
/// for one name as it would read for another.
fn error_for(response: &Value, from: &str, to: &str) -> String {
    response["error"].to_string().replace(from, to)
}

/// An active record of a member private to a person lies under that person
/// and the member's name. The member's owner and an administrator retire
/// it by the member's name, as the operator; the row stays where it was,
/// with its text. A second call retires nothing.
#[tokio::test]
async fn console_forget_retires_a_private_members_record_for_its_owner_and_an_administrator()
-> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-forget-owner-kept-{index}")).await?;
        for id in ["live-private", "live-private-2"] {
            fixture
                .seed(id, alices_scope(PRIVATE), "Home address", "12 Example Lane")
                .await;
        }
        let controller = AccessController::new(config)?;
        record_member(&controller, PRIVATE, Some(ALICE_KEY));
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        let own = fixture
            .rpc_as(&alice, FORGET, named(PRIVATE, "live-private"))
            .await;
        assert!(own["error"].is_null(), "rules on {enabled}: {own:#?}");
        assert_eq!(
            own["result"],
            deleted("live-private"),
            "rules on {enabled}: {own:#?}"
        );
        let retired = fixture.record("live-private").await;
        assert_eq!(retired.status, RecordStatus::Tombstoned);
        assert_eq!(retired.scope, alices_scope(PRIVATE), "the row stays");
        assert_eq!(retired.body, "12 Example Lane", "retiring does not erase");
        assert_eq!(
            fixture.audit_authors("live-private", "tombstone"),
            vec![json!({ "author": "operator", "principal": ALICE })],
            "rules on {enabled}"
        );
        let again = fixture
            .rpc_as(&alice, FORGET, named(PRIVATE, "live-private"))
            .await;
        assert_eq!(
            again["result"],
            json!({ "memory_id": "live-private", "deleted": false }),
            "rules on {enabled}: {again:#?}"
        );

        let administrators = fixture
            .rpc_as(&admin, FORGET, named(PRIVATE, "live-private-2"))
            .await;
        assert_eq!(
            administrators["result"],
            deleted("live-private-2"),
            "rules on {enabled}: {administrators:#?}"
        );
        assert_eq!(
            fixture.record("live-private-2").await.status,
            RecordStatus::Tombstoned
        );
    }
    Ok(())
}

/// A caller who is neither the member's owner nor an administrator gets,
/// for a member private to another person, for a member that ended and
/// that the Console holds nothing about, and for a name no member ever
/// held, one answer: the answer for a name that holds no such record, with
/// the name as the request spelled it. So does the owner of the ended
/// member: a member the Console does not know is answered to her as to
/// everybody who is no administrator. Nothing is retired. An administrator
/// retires the ended member's record.
#[tokio::test]
async fn console_forget_answers_alike_for_a_hidden_an_ended_and_an_absent_member() -> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture = ConsoleMemoryFixture::new(&format!("console-forget-absent-{index}")).await?;
        fixture
            .seed(
                "live-private",
                alices_scope(PRIVATE),
                "Home address",
                "12 Example Lane",
            )
            .await;
        fixture
            .seed(
                "live-ended",
                alices_scope(ENDED_PRIVATE),
                "Home address",
                "12 Example Lane",
            )
            .await;
        let controller = AccessController::new(config)?;
        record_member(&controller, PRIVATE, Some(ALICE_KEY));
        let bob = ConsoleCaller::person(&controller, BOB);
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        let absent = fixture
            .rpc_as(&bob, FORGET, named(NOBODY, "live-nobody"))
            .await;
        assert!(
            absent["result"].is_null() && !absent["error"].is_null(),
            "rules on {enabled}: {absent:#?}"
        );
        for (caller, who, member, id) in [
            (&bob, "another person", PRIVATE, "live-private"),
            (&bob, "another person", ENDED_PRIVATE, "live-ended"),
            (&alice, "the owner", ENDED_PRIVATE, "live-ended"),
        ] {
            let response = fixture.rpc_as(caller, FORGET, named(member, id)).await;
            assert!(
                response["result"].is_null(),
                "rules on {enabled}, {who}, {member}: {response:#?}"
            );
            assert_eq!(
                response["error"].to_string(),
                error_for(&absent, NOBODY, member),
                "rules on {enabled}, {who}, {member}: one answer for every such name"
            );
            assert_eq!(
                fixture.record(id).await.status,
                RecordStatus::Active,
                "rules on {enabled}, {who}, {member}"
            );
        }

        let administrators = fixture
            .rpc_as(&admin, FORGET, named(ENDED_PRIVATE, "live-ended"))
            .await;
        assert_eq!(
            administrators["result"],
            deleted("live-ended"),
            "rules on {enabled}: {administrators:#?}"
        );
        let retired = fixture.record("live-ended").await;
        assert_eq!(retired.status, RecordStatus::Tombstoned);
        assert_eq!(retired.scope, alices_scope(ENDED_PRIVATE), "the row stays");
    }
    Ok(())
}

/// A name a private member held can be held by another member later, and
/// the old member's records stay under its owner. A caller who names the
/// member that holds the name now, with the id of such a record, is
/// answered exactly as for an id no record holds, and the record stays:
/// the id opens nothing. The old member's owner and an administrator
/// retire the old record under the name.
#[tokio::test]
async fn console_forget_keeps_a_private_members_record_from_a_caller_who_names_another_member_of_its_name()
-> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-forget-reused-name-{index}")).await?;
        for id in ["live-reused-a", "live-reused-b"] {
            fixture
                .seed(id, alices_scope(REUSED), "Home address", "12 Example Lane")
                .await;
        }
        let controller = AccessController::new(config)?;
        // The member that holds the name now is private to nobody.
        record_member(&controller, REUSED, None);
        let bob = ConsoleCaller::person(&controller, BOB);
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        let no_such_record = fixture
            .rpc_as(&bob, FORGET, named(REUSED, "live-nowhere"))
            .await;
        for id in ["live-reused-a", "live-reused-b"] {
            let response = fixture.rpc_as(&bob, FORGET, named(REUSED, id)).await;
            assert_eq!(
                response.to_string().replace(id, "live-nowhere"),
                no_such_record.to_string(),
                "rules on {enabled}, {id}: the id opens nothing"
            );
            assert_eq!(
                fixture.record(id).await.status,
                RecordStatus::Active,
                "rules on {enabled}, {id}"
            );
        }

        let owners = fixture
            .rpc_as(&alice, FORGET, named(REUSED, "live-reused-a"))
            .await;
        assert_eq!(
            owners["result"],
            deleted("live-reused-a"),
            "rules on {enabled}: {owners:#?}"
        );
        let administrators = fixture
            .rpc_as(&admin, FORGET, named(REUSED, "live-reused-b"))
            .await;
        assert_eq!(
            administrators["result"],
            deleted("live-reused-b"),
            "rules on {enabled}: {administrators:#?}"
        );
        for id in ["live-reused-a", "live-reused-b"] {
            let retired = fixture.record(id).await;
            assert_eq!(
                retired.status,
                RecordStatus::Tombstoned,
                "rules on {enabled}, {id}"
            );
            assert_eq!(retired.scope, alices_scope(REUSED), "the row stays");
        }
    }
    Ok(())
}
