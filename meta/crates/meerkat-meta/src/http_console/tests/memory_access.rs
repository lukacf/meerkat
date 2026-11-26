//! Rows that drive `meta/agent_memory/purge` on the Console through a
//! caller's access view: the grant end to end, and the rule that answers a
//! member the caller may not know of as a member that does not exist.

use super::memory_fixture::{
    ConsoleCaller, ConsoleMemoryFixture, MOB, REALM, RowResult, mob_scope, record_member,
};
use super::*;
use crate::access::{AccessControlConfig, AccessRule};
use crate::memory::records::{MemoryScope, RecordStatus};

const ADMIN: &str = "admin@example.test";
const ALICE: &str = "alice@example.test";
const ALICE_KEY: &str = "console-caller:v1:alice";
const BOB: &str = "bob@example.test";
const CAROL: &str = "carol@example.test";

/// A member every caller may know.
const SHARED: &str = "identity:shared";
/// A second member every caller may know.
const OTHER: &str = "identity:other";
/// A member private to Alice.
const PRIVATE: &str = "identity:private";
/// A member that ended before the Console read who it was private to: the
/// Console holds nothing about it, and the store still holds its records.
const ENDED: &str = "identity:ended";
/// A name no member ever held.
const NOBODY: &str = "identity:nobody";
/// A name a member private to Alice held, and a member every caller may
/// know holds now.
const REUSED: &str = "identity:reused";
/// A member private to Alice that ended; the Console holds nothing about
/// it, and the store keeps its records under her and its name.
const ENDED_PRIVATE: &str = "identity:ended-private";

const PURGE: &str = "meta/agent_memory/purge";

fn identity_scope(identity: &str) -> MemoryScope {
    MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: identity.to_string(),
    }
}

/// The scope that keeps the memory of the member `member` private to
/// Alice: under her and the member's name.
fn alices_scope(member: &str) -> MemoryScope {
    identity_scope(&crate::memory::records::private_member_scope_key(
        ALICE_KEY, member,
    ))
}

/// The scope a seeded member's records lie in: under Alice for the member
/// private to her, under the name for every other.
fn scope_of(member: &str) -> MemoryScope {
    if member == PRIVATE {
        alices_scope(member)
    } else {
        identity_scope(member)
    }
}

fn identity_params(identity: &str, memory_id: &str) -> Value {
    json!({ "realm": REALM, "identity": identity, "memory_id": memory_id })
}

fn mob_params(memory_id: &str) -> Value {
    json!({ "realm": REALM, "scope": { "kind": "mob", "key": MOB }, "memory_id": memory_id })
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

/// The id of the retired record seeded in `member`'s scope.
fn gone(member: &str) -> String {
    format!("gone-{}", member.trim_start_matches("identity:"))
}

/// The id of the active record seeded in `member`'s scope.
fn live(member: &str) -> String {
    format!("live-{}", member.trim_start_matches("identity:"))
}

/// The answer a purge gives for a record that is not there.
fn nothing(memory_id: &str) -> Value {
    json!({ "memory_id": memory_id, "purged": false, "purged_ids": [] })
}

/// A purge's answer for the one record it blanked.
fn purged(memory_id: &str) -> Value {
    json!({ "memory_id": memory_id, "purged": true, "purged_ids": [memory_id] })
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

/// Each member's scope holds a retired record `gone-<member>` and an active
/// record `live-<member>`; the name no member held holds none. The member
/// private to Alice has its records where the store keeps them: under her
/// and its name.
async fn seed_members(fixture: &ConsoleMemoryFixture) {
    for member in [SHARED, PRIVATE, ENDED] {
        fixture
            .seed_retired(
                &gone(member),
                scope_of(member),
                "Home address",
                "12 Example Lane",
            )
            .await;
        fixture
            .seed(
                &live(member),
                scope_of(member),
                "Favourite tea",
                "Assam, no milk",
            )
            .await;
    }
}

/// The access configurations a Console that can identify a person runs
/// with: rules on, where Bob holds the purge grant on every agent and
/// Alice on her own and on the name her ended member held, and rules off.
fn configurations() -> [AccessControlConfig; 2] {
    [
        AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![
                allow(
                    "bob-purges-any-agent",
                    BOB,
                    &[crate::access::ACTION_AGENT_MEMORY_ADMIN],
                    &["*"],
                ),
                allow(
                    "alice-purges-her-agents",
                    ALICE,
                    &[crate::access::ACTION_AGENT_MEMORY_ADMIN],
                    &[PRIVATE, REUSED],
                ),
            ],
            ..AccessControlConfig::default()
        },
        AccessControlConfig {
            enabled: false,
            admins: vec![ADMIN.to_string()],
            ..AccessControlConfig::default()
        },
    ]
}

/// A caller who is neither the member's owner nor an administrator gets,
/// for a member private to another person, for a member the Console no
/// longer knows and for a name no member ever held, one answer: the
/// answer for a record that is not there. Nothing is blanked, and nothing
/// says a record of that member exists or what its status is. The caller
/// here holds the purge grant on every agent, so the grant is not what
/// stops it.
#[tokio::test]
async fn console_purge_answers_a_member_the_caller_may_not_know_as_absent() -> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture = ConsoleMemoryFixture::new(&format!("console-purge-unknown-{index}")).await?;
        seed_members(&fixture).await;
        let controller = AccessController::new(config)?;
        record_member(&controller, SHARED, None);
        record_member(&controller, PRIVATE, Some(ALICE_KEY));
        let bob = ConsoleCaller::person(&controller, BOB);

        for member in [PRIVATE, ENDED, NOBODY] {
            for memory_id in [gone(member), live(member)] {
                let response = fixture
                    .rpc_as(&bob, PURGE, identity_params(member, &memory_id))
                    .await;
                assert!(
                    response["error"].is_null(),
                    "rules on {enabled}, {member}: {response:#?}"
                );
                assert_eq!(
                    response["result"],
                    nothing(&memory_id),
                    "rules on {enabled}, {member}: {response:#?}"
                );
            }
        }
        for member in [PRIVATE, ENDED] {
            let kept = fixture.record(&gone(member)).await;
            assert_eq!(
                (kept.title.as_str(), kept.body.as_str()),
                ("Home address", "12 Example Lane"),
                "rules on {enabled}, {member}"
            );
        }

        // A member the Console knows and shares is withheld from nobody.
        let shared = fixture
            .rpc_as(&bob, PURGE, identity_params(SHARED, &gone(SHARED)))
            .await;
        assert_eq!(
            shared["result"]["purged"],
            json!(true),
            "rules on {enabled}: {shared:#?}"
        );
    }
    Ok(())
}

/// The member's owner and an administrator are withheld nothing: the owner
/// purges a record of the member private to them, and an administrator one
/// of a member the Console no longer knows.
#[tokio::test]
async fn console_purge_serves_the_owner_and_an_administrator() -> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture = ConsoleMemoryFixture::new(&format!("console-purge-owner-{index}")).await?;
        seed_members(&fixture).await;
        let controller = AccessController::new(config)?;
        record_member(&controller, SHARED, None);
        record_member(&controller, PRIVATE, Some(ALICE_KEY));

        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let own = fixture
            .rpc_as(&alice, PURGE, identity_params(PRIVATE, &gone(PRIVATE)))
            .await;
        assert_eq!(
            own["result"]["purged"],
            json!(true),
            "rules on {enabled}: {own:#?}"
        );

        let admin = ConsoleCaller::person(&controller, ADMIN);
        let ended = fixture
            .rpc_as(&admin, PURGE, identity_params(ENDED, &gone(ENDED)))
            .await;
        assert_eq!(
            ended["result"]["purged"],
            json!(true),
            "rules on {enabled}: {ended:#?}"
        );
        assert_eq!(fixture.record(&gone(ENDED)).await.body, "");
    }
    Ok(())
}

/// A caller without the purge grant is refused alike for a member private
/// to another person, a member the Console no longer knows and a name no
/// member held: the same refusal in its code, its words and every field,
/// naming the member as the request spelled it, whatever the store holds.
#[tokio::test]
async fn console_purge_refuses_a_caller_without_the_grant_alike_for_every_unknown_member()
-> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge-unknown-denied").await?;
    seed_members(&fixture).await;
    let [config, _] = configurations();
    let controller = AccessController::new(config)?;
    record_member(&controller, SHARED, None);
    record_member(&controller, PRIVATE, Some(ALICE_KEY));
    let carol = ConsoleCaller::person(&controller, CAROL);

    for member in [PRIVATE, ENDED, NOBODY] {
        let response = fixture
            .rpc_as(&carol, PURGE, identity_params(member, &gone(member)))
            .await;
        assert!(response["result"].is_null(), "{member}: {response:#?}");
        assert_eq!(
            response["error"],
            json!({
                "code": -32030,
                "message": format!(
                    "access denied: {}",
                    crate::access::ACTION_AGENT_MEMORY_ADMIN
                ),
                "data": {
                    "kind": "access_denied",
                    "action": crate::access::ACTION_AGENT_MEMORY_ADMIN,
                    "resource": member,
                },
            }),
            "{member}: {response:#?}"
        );
    }
    for member in [PRIVATE, ENDED] {
        assert_eq!(fixture.record(&gone(member)).await.body, "12 Example Lane");
    }
    Ok(())
}

/// The grant end to end. `agent.memory.admin` on one agent purges that
/// agent's retired record and nothing else: not another agent's, and not a
/// shared record, which needs the grant with no agent named. A caller with
/// no grant purges nothing.
#[tokio::test]
async fn console_purge_enforces_its_grant_per_agent_and_unscoped_for_a_shared_record() -> RowResult
{
    let fixture = ConsoleMemoryFixture::new("console-purge-grants").await?;
    for member in [SHARED, OTHER] {
        fixture
            .seed_retired(
                &gone(member),
                identity_scope(member),
                "Home address",
                "12 Example Lane",
            )
            .await;
    }
    fixture
        .seed_retired(
            "gone-mob",
            mob_scope(),
            "Customer note",
            "a customer's detail",
        )
        .await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            allow(
                "bob-purges-one-agent",
                BOB,
                &[crate::access::ACTION_AGENT_MEMORY_ADMIN],
                &[SHARED],
            ),
            allow(
                "alice-purges-anywhere",
                ALICE,
                &[crate::access::ACTION_AGENT_MEMORY_ADMIN],
                &[],
            ),
        ],
        ..AccessControlConfig::default()
    })?;
    record_member(&controller, SHARED, None);
    record_member(&controller, OTHER, None);
    let bob = ConsoleCaller::person(&controller, BOB);
    let alice = ConsoleCaller::person(&controller, ALICE);
    let carol = ConsoleCaller::person(&controller, CAROL);
    let intact = |record: crate::memory::records::MemoryRecord| {
        assert_eq!(record.status, RecordStatus::Tombstoned);
        assert_ne!(record.body, "", "{record:#?}");
    };

    // No grant: nothing, anywhere.
    for params in [
        identity_params(SHARED, &gone(SHARED)),
        mob_params("gone-mob"),
    ] {
        let response = fixture.rpc_as(&carol, PURGE, params).await;
        assert_denied(&response, crate::access::ACTION_AGENT_MEMORY_ADMIN);
    }
    // The grant on one agent: not another agent's record, not a shared one.
    let cross = fixture
        .rpc_as(&bob, PURGE, identity_params(OTHER, &gone(OTHER)))
        .await;
    assert_denied(&cross, crate::access::ACTION_AGENT_MEMORY_ADMIN);
    let shared = fixture.rpc_as(&bob, PURGE, mob_params("gone-mob")).await;
    assert_denied(&shared, crate::access::ACTION_AGENT_MEMORY_ADMIN);
    for id in [gone(SHARED), gone(OTHER), "gone-mob".to_string()] {
        intact(fixture.record(&id).await);
    }

    // The grant on the agent named: that agent's record.
    let own = fixture
        .rpc_as(&bob, PURGE, identity_params(SHARED, &gone(SHARED)))
        .await;
    assert_eq!(own["result"]["purged"], json!(true), "{own:#?}");
    // The grant with no agent named: a shared record, and any agent's.
    let mob = fixture.rpc_as(&alice, PURGE, mob_params("gone-mob")).await;
    assert_eq!(mob["result"]["purged"], json!(true), "{mob:#?}");
    let other = fixture
        .rpc_as(&alice, PURGE, identity_params(OTHER, &gone(OTHER)))
        .await;
    assert_eq!(other["result"]["purged"], json!(true), "{other:#?}");
    assert_eq!(
        fixture.audit_authors("gone-mob", "purge"),
        vec![json!({ "author": "operator", "principal": ALICE })]
    );
    Ok(())
}

/// The Console offers the purge only to a caller who could use it
/// somewhere: one who holds its grant on some agent or with no agent
/// named. A caller without the grant is not offered an action the first
/// click would refuse.
#[tokio::test]
async fn console_advertises_purge_only_to_a_caller_who_holds_its_grant() -> RowResult {
    let fixture = ConsoleMemoryFixture::new("console-purge-advertised-grant").await?;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ADMIN.to_string()],
        rules: vec![
            allow(
                "bob-purges-one-agent",
                BOB,
                &[crate::access::ACTION_AGENT_MEMORY_ADMIN],
                &[SHARED],
            ),
            allow(
                "carol-forgets-anywhere",
                CAROL,
                &[
                    crate::access::ACTION_AGENT_MEMORY_DELETE,
                    crate::access::ACTION_AGENT_MEMORY_WRITE,
                ],
                &[],
            ),
        ],
        ..AccessControlConfig::default()
    })?;
    record_member(&controller, SHARED, None);
    let method = PURGE.to_string();

    let bob = ConsoleCaller::person(&controller, BOB);
    assert!(fixture.advertised_to(&bob).await.contains(&method));
    let admin = ConsoleCaller::person(&controller, ADMIN);
    assert!(fixture.advertised_to(&admin).await.contains(&method));
    let carol = ConsoleCaller::person(&controller, CAROL);
    let offered = fixture.advertised_to(&carol).await;
    assert!(!offered.contains(&method), "{offered:#?}");
    assert!(
        offered.contains(&"meta/agent_memory/forget".to_string()),
        "the forget its grant covers is still offered: {offered:#?}"
    );
    Ok(())
}

/// A record of a member private to a person lies under that person and
/// the member's name. The member's owner and an administrator purge it
/// by the member's name, and the row stays where it was. The owner's
/// request is judged on the record itself: a record that is not retired
/// is refused as one, where a caller the member is kept from is
/// answered as for a record that is not there.
#[tokio::test]
async fn console_purge_reaches_a_private_members_record_for_its_owner_and_an_administrator()
-> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-purge-owner-kept-{index}")).await?;
        seed_members(&fixture).await;
        fixture
            .seed_retired(
                "gone-private-2",
                alices_scope(PRIVATE),
                "Old address",
                "3 Example Row",
            )
            .await;
        let controller = AccessController::new(config)?;
        record_member(&controller, SHARED, None);
        record_member(&controller, PRIVATE, Some(ALICE_KEY));
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);
        let bob = ConsoleCaller::person(&controller, BOB);

        let own = fixture
            .rpc_as(&alice, PURGE, identity_params(PRIVATE, &gone(PRIVATE)))
            .await;
        assert!(own["error"].is_null(), "rules on {enabled}: {own:#?}");
        assert_eq!(
            own["result"],
            purged(&gone(PRIVATE)),
            "rules on {enabled}: {own:#?}"
        );
        let blanked = fixture.record(&gone(PRIVATE)).await;
        assert_eq!(
            (blanked.title.as_str(), blanked.body.as_str()),
            ("", ""),
            "rules on {enabled}"
        );
        assert_eq!(blanked.status, RecordStatus::Tombstoned);
        assert_eq!(blanked.scope, alices_scope(PRIVATE), "the row stays");
        assert_eq!(
            fixture.audit_authors(&gone(PRIVATE), "purge"),
            vec![json!({ "author": "operator", "principal": ALICE })],
            "rules on {enabled}"
        );

        let administrators = fixture
            .rpc_as(&admin, PURGE, identity_params(PRIVATE, "gone-private-2"))
            .await;
        assert_eq!(
            administrators["result"],
            purged("gone-private-2"),
            "rules on {enabled}: {administrators:#?}"
        );
        assert_eq!(fixture.record("gone-private-2").await.body, "");

        // The record that is not retired: the owner is told so, and the
        // caller the member is kept from is told nothing.
        let not_retired = fixture
            .rpc_as(&alice, PURGE, identity_params(PRIVATE, &live(PRIVATE)))
            .await;
        assert_eq!(
            not_retired["error"]["data"]["kind"],
            json!(crate::identity_first::AgentMemoryPurgeError::NOT_TOMBSTONED_KIND),
            "rules on {enabled}: {not_retired:#?}"
        );
        let kept_from = fixture
            .rpc_as(&bob, PURGE, identity_params(PRIVATE, &live(PRIVATE)))
            .await;
        assert_eq!(
            kept_from["result"],
            nothing(&live(PRIVATE)),
            "rules on {enabled}: {kept_from:#?}"
        );
        assert_eq!(fixture.record(&live(PRIVATE)).await.body, "Assam, no milk");
    }
    Ok(())
}

/// A name a private member held can be held by another member later.
/// The old member's records stay under its owner. A caller who holds
/// the purge grant on every agent and names the member that holds the
/// name now, with the id of such a record, is answered as for a record
/// that is not there, and nothing is blanked: the id opens nothing. The
/// record of the member that holds the name now is purged for that
/// caller as before. The old member's owner and an administrator purge
/// the old record under the name.
#[tokio::test]
async fn console_purge_keeps_a_private_members_record_from_a_caller_who_names_another_member_of_its_name()
-> RowResult {
    for (index, config) in configurations().into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-purge-reused-name-{index}")).await?;
        for id in ["gone-reused-a", "gone-reused-b"] {
            fixture
                .seed_retired(id, alices_scope(REUSED), "Home address", "12 Example Lane")
                .await;
        }
        fixture
            .seed_retired(
                &gone(REUSED),
                identity_scope(REUSED),
                "Office address",
                "1 Example Square",
            )
            .await;
        let controller = AccessController::new(config)?;
        // The member that holds the name now is private to nobody.
        record_member(&controller, REUSED, None);
        let bob = ConsoleCaller::person(&controller, BOB);
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        for id in ["gone-reused-a", "gone-reused-b"] {
            let response = fixture
                .rpc_as(&bob, PURGE, identity_params(REUSED, id))
                .await;
            assert!(
                response["error"].is_null(),
                "rules on {enabled}, {id}: {response:#?}"
            );
            assert_eq!(
                response["result"],
                nothing(id),
                "rules on {enabled}, {id}: {response:#?}"
            );
            let kept = fixture.record(id).await;
            assert_eq!(
                (kept.title.as_str(), kept.body.as_str()),
                ("Home address", "12 Example Lane"),
                "rules on {enabled}, {id}"
            );
        }
        let current = fixture
            .rpc_as(&bob, PURGE, identity_params(REUSED, &gone(REUSED)))
            .await;
        assert_eq!(
            current["result"],
            purged(&gone(REUSED)),
            "rules on {enabled}: {current:#?}"
        );

        let owners = fixture
            .rpc_as(&alice, PURGE, identity_params(REUSED, "gone-reused-a"))
            .await;
        assert_eq!(
            owners["result"],
            purged("gone-reused-a"),
            "rules on {enabled}: {owners:#?}"
        );
        let administrators = fixture
            .rpc_as(&admin, PURGE, identity_params(REUSED, "gone-reused-b"))
            .await;
        assert_eq!(
            administrators["result"],
            purged("gone-reused-b"),
            "rules on {enabled}: {administrators:#?}"
        );
        for id in ["gone-reused-a", "gone-reused-b"] {
            let blanked = fixture.record(id).await;
            assert_eq!(blanked.body, "", "rules on {enabled}, {id}");
            assert_eq!(blanked.scope, alices_scope(REUSED), "the row stays");
        }
    }
    Ok(())
}

/// The fact as it is: when a private member has ended and the Console
/// holds nothing about it, the person it was private to is answered as
/// for a member that does not exist, with the purge grant on every
/// agent and with the rules off, and nothing is blanked. An
/// administrator purges that record under the member's name.
#[tokio::test]
async fn console_purge_answers_the_owner_of_an_ended_private_member_as_absent_and_serves_an_administrator()
-> RowResult {
    let configurations = [
        AccessControlConfig {
            enabled: true,
            admins: vec![ADMIN.to_string()],
            rules: vec![allow(
                "alice-purges-any-agent",
                ALICE,
                &[crate::access::ACTION_AGENT_MEMORY_ADMIN],
                &["*"],
            )],
            ..AccessControlConfig::default()
        },
        AccessControlConfig {
            enabled: false,
            admins: vec![ADMIN.to_string()],
            ..AccessControlConfig::default()
        },
    ];
    for (index, config) in configurations.into_iter().enumerate() {
        let enabled = config.enabled;
        let fixture =
            ConsoleMemoryFixture::new(&format!("console-purge-ended-private-{index}")).await?;
        let id = gone(ENDED_PRIVATE);
        fixture
            .seed_retired(
                &id,
                alices_scope(ENDED_PRIVATE),
                "Home address",
                "12 Example Lane",
            )
            .await;
        let controller = AccessController::new(config)?;
        record_member(&controller, SHARED, None);
        let alice = ConsoleCaller::owner(&controller, ALICE, ALICE_KEY);
        let admin = ConsoleCaller::person(&controller, ADMIN);

        let owners = fixture
            .rpc_as(&alice, PURGE, identity_params(ENDED_PRIVATE, &id))
            .await;
        assert!(owners["error"].is_null(), "rules on {enabled}: {owners:#?}");
        assert_eq!(
            owners["result"],
            nothing(&id),
            "rules on {enabled}: {owners:#?}"
        );
        assert_eq!(
            fixture.record(&id).await.body,
            "12 Example Lane",
            "rules on {enabled}"
        );

        let administrators = fixture
            .rpc_as(&admin, PURGE, identity_params(ENDED_PRIVATE, &id))
            .await;
        assert_eq!(
            administrators["result"],
            purged(&id),
            "rules on {enabled}: {administrators:#?}"
        );
        let blanked = fixture.record(&id).await;
        assert_eq!(blanked.body, "", "rules on {enabled}");
        assert_eq!(blanked.scope, alices_scope(ENDED_PRIVATE), "the row stays");
    }
    Ok(())
}

/// The grant through a wildcard, each held by a caller with no other rule
/// and with no agent named. A pattern that covers `agent.memory.admin`
/// (`agent.memory.*`, `agent.*`, `*`) purges an agent's retired record and
/// a shared one, as the action named exactly does. A pattern of the mob
/// family (`mob.memory.*`, `mob.*`) covers no agent action and purges
/// neither. The purge of a shared record needs the grant with no resource
/// named: a rule that selects a resource, even the agents of every mob,
/// purges the record of an agent it selects and no shared one.
#[tokio::test]
async fn console_purge_is_granted_by_each_wildcard_that_covers_agent_memory_admin() -> RowResult {
    /// (the caller, its one pattern, covers `agent.memory.admin`)
    const CALLERS: [(&str, &str, bool); 5] = [
        ("dana@example.test", "agent.memory.*", true),
        ("erin@example.test", "agent.*", true),
        ("frank@example.test", "*", true),
        ("gina@example.test", "mob.memory.*", false),
        ("hank@example.test", "mob.*", false),
    ];
    /// The caller whose one rule holds every action for the agents of
    /// every mob: a rule with a resource selector.
    const SELECTING: &str = "ivan@example.test";
    let fixture = ConsoleMemoryFixture::new("console-purge-wildcards").await?;
    for index in 0..=CALLERS.len() {
        fixture
            .seed_retired(
                &format!("gone-agent-{index}"),
                identity_scope(SHARED),
                &format!("Home address {index}"),
                &format!("{index} Example Lane"),
            )
            .await;
        fixture
            .seed_retired(
                &format!("gone-mob-{index}"),
                mob_scope(),
                &format!("Customer note {index}"),
                &format!("a detail of customer {index}"),
            )
            .await;
    }
    let mut rules: Vec<AccessRule> = CALLERS
        .iter()
        .enumerate()
        .map(|(index, (subject, pattern, _))| {
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
    record_member(&controller, SHARED, None);

    for (index, (subject, pattern, covers)) in CALLERS.iter().enumerate() {
        let caller = ConsoleCaller::person(&controller, subject);
        let agent_id = format!("gone-agent-{index}");
        let mob_id = format!("gone-mob-{index}");
        let of_the_agent = fixture
            .rpc_as(&caller, PURGE, identity_params(SHARED, &agent_id))
            .await;
        let of_the_mob = fixture.rpc_as(&caller, PURGE, mob_params(&mob_id)).await;
        if *covers {
            assert_eq!(
                of_the_agent["result"],
                purged(&agent_id),
                "{pattern}: {of_the_agent:#?}"
            );
            assert_eq!(
                of_the_mob["result"],
                purged(&mob_id),
                "{pattern}: {of_the_mob:#?}"
            );
        } else {
            assert_denied(&of_the_agent, crate::access::ACTION_AGENT_MEMORY_ADMIN);
            assert_denied(&of_the_mob, crate::access::ACTION_AGENT_MEMORY_ADMIN);
        }
        for id in [&agent_id, &mob_id] {
            let record = fixture.record(id).await;
            assert_eq!(record.status, RecordStatus::Tombstoned, "{pattern}: {id}");
            assert_eq!(
                record.body.is_empty() && record.title.is_empty(),
                *covers,
                "{pattern}: {record:#?}"
            );
        }
    }

    // The rule that selects a resource: the record of an agent it selects,
    // and no shared record.
    let selecting = ConsoleCaller::person(&controller, SELECTING);
    let agent_id = format!("gone-agent-{}", CALLERS.len());
    let mob_id = format!("gone-mob-{}", CALLERS.len());
    let of_the_agent = fixture
        .rpc_as(&selecting, PURGE, identity_params(SHARED, &agent_id))
        .await;
    assert_eq!(
        of_the_agent["result"],
        purged(&agent_id),
        "{of_the_agent:#?}"
    );
    let of_the_mob = fixture.rpc_as(&selecting, PURGE, mob_params(&mob_id)).await;
    assert_denied(&of_the_mob, crate::access::ACTION_AGENT_MEMORY_ADMIN);
    let kept = fixture.record(&mob_id).await;
    assert_eq!(kept.status, RecordStatus::Tombstoned, "{kept:#?}");
    assert_ne!(kept.body, "", "{kept:#?}");
    Ok(())
}
