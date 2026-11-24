//! Shared console objects one caller cannot quietly take from another: an
//! access edit is written only against the revision its editor read, and on
//! a console without an access controller a caller revokes only their own
//! sessions. A revoke that names no principal revokes the caller's own on
//! every console.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{
    console_rpc_as, durable_fences, human_claims, now_seconds, token,
};
use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const ROBOT: &str = "svc:robot";

/// A token of the Console human BOB, issued a minute ago.
fn bob() -> String {
    let mut claims = human_claims();
    claims["iat"] = json!(now_seconds() - 60);
    token(claims, Some(now_seconds() + 3600))
}

/// Every access edit on the console names the revision and the owner it was
/// made against (`checked_v1`): a write without them is refused as
/// `revision_required` and changes nothing, so no client overwrites an edit
/// it never saw. The same edit in its checked form commits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn access_write_without_revision_refused() -> TestResult {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        groups: [
            (
                "ops".to_string(),
                AccessGroup {
                    description: None,
                    members: vec![ROBOT.to_string()],
                },
            ),
            ("unused".to_string(), AccessGroup::default()),
        ]
        .into(),
        rules: vec![AccessRule {
            id: "ops-view".to_string(),
            groups: vec!["ops".to_string()],
            actions: vec!["agent.view".to_string()],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .map_err(|error| format!("{error:?}"))?;
    let router = console_json_router_with_state(ConsoleJsonState {
        access: Some(access.clone()),
        ..state()
    });
    let bob = bob();
    let (config, revision) = access.snapshot();
    let send_rule = json!({ "id": "ops-send", "groups": ["ops"], "actions": ["agent.send"] });
    let unchecked = [
        (
            "meta/access/set",
            json!({ "config": serde_json::to_value(&*config)? }),
        ),
        ("meta/access/enable", json!({ "enabled": true })),
        ("meta/access/rules/upsert", json!({ "rule": send_rule })),
        ("meta/access/rules/delete", json!({ "id": "ops-view" })),
        (
            "meta/access/groups/set",
            json!({ "name": "ops", "group": { "members": [ROBOT, "eve@example.test"] } }),
        ),
        ("meta/access/groups/delete", json!({ "name": "unused" })),
    ];
    for (method, params) in unchecked {
        let answer = console_rpc_as(&router, Some(&bob), method, params).await;
        assert_eq!(
            (
                answer["error"]["code"].clone(),
                answer["error"]["data"]["kind"].clone()
            ),
            (json!(-32602), json!("revision_required")),
            "{method}: {answer:#}"
        );
        assert_eq!(access.snapshot().1, revision, "{method} changed nothing");
    }

    let owner = access
        .edit_identity()
        .ok_or("the access owner has an edit identity")?
        .to_string();
    let checked = console_rpc_as(
        &router,
        Some(&bob),
        "meta/access/rules/upsert",
        json!({
            "checked_v1": {
                "owner_instance": owner,
                "expected_revision": revision,
                "rule": send_rule,
            }
        }),
    )
    .await;
    assert!(checked["error"].is_null(), "{checked:#}");
    assert_eq!(checked["result"]["revision"], json!(revision + 1));
    Ok(())
}

/// On a console without an access controller a caller revokes only their
/// own sessions: naming another principal is refused and fences no one,
/// while naming themselves, or no one, fences their own sessions. A console
/// with an access controller keeps its rule: an access admin revokes anyone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_other_without_controller_refused() -> TestResult {
    let bob = bob();
    let (_dir, fences) = durable_fences();
    let router = console_json_router_with_state(ConsoleJsonState {
        session_fences: fences.clone(),
        ..state()
    });
    let refused = console_rpc_as(
        &router,
        Some(&bob),
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
        json!({ "principal": ROBOT }),
    )
    .await;
    assert_eq!(
        (
            refused["error"]["code"].clone(),
            refused["error"]["data"]["kind"].clone()
        ),
        (json!(ACCESS_DENIED_RPC_CODE), json!("access_denied")),
        "{refused:#}"
    );
    assert_eq!(fences.not_before(ROBOT), None, "no one else is fenced");

    let own = console_rpc_as(
        &router,
        Some(&bob),
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
        json!({ "principal": BOB }),
    )
    .await;
    assert!(own["error"].is_null(), "{own:#}");
    assert!(fences.not_before(BOB).is_some(), "the caller's own are");

    let (_other_dir, other_fences) = durable_fences();
    let router = console_json_router_with_state(ConsoleJsonState {
        session_fences: other_fences.clone(),
        ..state()
    });
    let unnamed = console_rpc_as(
        &router,
        Some(&bob),
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
        json!({}),
    )
    .await;
    assert!(unnamed["error"].is_null(), "{unnamed:#}");
    assert_eq!(unnamed["result"]["principal"], json!(BOB));
    assert!(other_fences.not_before(BOB).is_some());

    // An access admin revokes another principal where a controller is
    // installed, as before.
    let (_admin_dir, admin_fences) = durable_fences();
    let router = console_json_router_with_state(ConsoleJsonState {
        session_fences: admin_fences.clone(),
        access: Some(
            AccessController::new(crate::access::AccessControlConfig {
                enabled: true,
                admins: vec![BOB.to_string()],
                ..crate::access::AccessControlConfig::default()
            })
            .map_err(|error| format!("{error:?}"))?,
        ),
        ..state()
    });
    let revoked = console_rpc_as(
        &router,
        Some(&bob),
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
        json!({ "principal": ROBOT }),
    )
    .await;
    assert!(revoked["error"].is_null(), "{revoked:#}");
    assert!(admin_fences.not_before(ROBOT).is_some());
    Ok(())
}

/// A revoke that names no principal revokes the caller's own sessions on
/// every console: on one with an access controller too, where the caller is
/// an access admin who could have named anyone. It fences the admin alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admin_revoke_without_principal_revokes_own_sessions() -> TestResult {
    let bob = bob();
    for params in [json!({}), json!({ "principal": null })] {
        let (_dir, fences) = durable_fences();
        let router = console_json_router_with_state(ConsoleJsonState {
            session_fences: fences.clone(),
            access: Some(
                AccessController::new(crate::access::AccessControlConfig {
                    enabled: true,
                    admins: vec![BOB.to_string()],
                    ..crate::access::AccessControlConfig::default()
                })
                .map_err(|error| format!("{error:?}"))?,
            ),
            ..state()
        });
        let answer = console_rpc_as(
            &router,
            Some(&bob),
            crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
            params,
        )
        .await;
        assert!(answer["error"].is_null(), "{answer:#}");
        assert_eq!(answer["result"]["principal"], json!(BOB), "{answer:#}");
        assert!(
            fences.not_before(BOB).is_some(),
            "the admin's own are fenced"
        );
        assert_eq!(fences.not_before(ROBOT), None, "no one else is");
    }
    Ok(())
}

/// A rule that lists mobs keeps them through the console: the configuration
/// the console reads names the rule's `mobs`, and the rule saved back with
/// another edit (as the Console's rule editor sends it) still lists them. A
/// rule that lists none is read and saved without the field.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_access_a_rule_saved_through_the_console_keeps_its_mobs() -> TestResult {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        rules: vec![
            AccessRule {
                id: "harbor-desks".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec!["agent.view".to_string()],
                mobs: vec!["harbor".to_string(), "quay".to_string()],
                ..AccessRule::default()
            },
            AccessRule {
                id: "desks".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec!["agent.view".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .map_err(|error| format!("{error:?}"))?;
    let router = console_json_router_with_state(ConsoleJsonState {
        access: Some(access.clone()),
        ..state()
    });
    let bob = bob();
    let read = console_rpc_as(&router, Some(&bob), "meta/access/get", json!({})).await;
    assert!(read["error"].is_null(), "{read:#}");
    let rules = read["result"]["config"]["rules"].clone();
    assert_eq!(rules[0]["mobs"], json!(["harbor", "quay"]), "{read:#}");
    assert!(rules[1].get("mobs").is_none(), "{read:#}");

    let owner = access
        .edit_identity()
        .ok_or("the access owner has an edit identity")?
        .to_string();
    let mut revision = read["result"]["revision"]
        .as_u64()
        .ok_or("the read names a revision")?;
    for (index, id) in [(0, "harbor-desks"), (1, "desks")] {
        let mut edited = rules[index].clone();
        edited["actions"] = json!(["agent.view", "agent.send"]);
        edited["description"] = json!(format!("{id}, edited"));
        let saved = console_rpc_as(
            &router,
            Some(&bob),
            "meta/access/rules/upsert",
            json!({
                "checked_v1": {
                    "owner_instance": owner,
                    "expected_revision": revision,
                    "rule": edited,
                }
            }),
        )
        .await;
        assert!(saved["error"].is_null(), "{id}: {saved:#}");
        revision += 1;
        assert_eq!(saved["result"]["revision"], json!(revision), "{id}");
    }

    let (config, _) = access.snapshot();
    assert_eq!(config.rules[0].mobs, ["harbor", "quay"]);
    assert!(
        config.rules[0]
            .actions
            .iter()
            .any(|action| action == "agent.send")
    );
    assert_eq!(
        config.rules[0].description.as_deref(),
        Some("harbor-desks, edited")
    );
    assert!(config.rules[1].mobs.is_empty());
    let reread = console_rpc_as(&router, Some(&bob), "meta/access/get", json!({})).await;
    let rules = &reread["result"]["config"]["rules"];
    assert_eq!(rules[0]["mobs"], json!(["harbor", "quay"]), "{reread:#}");
    assert_eq!(rules[0]["description"], json!("harbor-desks, edited"));
    assert!(rules[1].get("mobs").is_none(), "{reread:#}");
    Ok(())
}

/// The runtime has no create-only access write: `rules/upsert` under the ID
/// of an existing rule replaces that rule whole, the mobs it lists included.
/// What keeps a client's new rule from replacing a rule the client has not
/// seen is the revision its write is made against: a rule created since
/// that revision makes the write a conflict, and the rule stays as it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_keyed_access_a_new_rule_does_not_replace_a_rule_created_since_its_read()
-> TestResult {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![BOB.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .map_err(|error| format!("{error:?}"))?;
    let router = console_json_router_with_state(ConsoleJsonState {
        access: Some(access.clone()),
        ..state()
    });
    let bob = bob();
    let owner = access
        .edit_identity()
        .ok_or("the access owner has an edit identity")?
        .to_string();
    let read = console_rpc_as(&router, Some(&bob), "meta/access/get", json!({})).await;
    let seen = read["result"]["revision"]
        .as_u64()
        .ok_or("the read names a revision")?;
    let upsert = |expected_revision: u64, rule: serde_json::Value| {
        console_rpc_as(
            &router,
            Some(&bob),
            "meta/access/rules/upsert",
            json!({
                "checked_v1": {
                    "owner_instance": owner,
                    "expected_revision": expected_revision,
                    "rule": rule,
                }
            }),
        )
    };
    let for_the_harbor = json!({
        "id": "harbor-desks",
        "subjects": ["*"],
        "actions": ["agent.view"],
        "mobs": ["harbor"],
        "description": "for the harbor",
    });
    let new_rule = json!({
        "id": "harbor-desks",
        "subjects": ["*"],
        "actions": ["agent.view"],
        "description": "a new rule",
    });

    // Someone else creates the rule after the client's read.
    let created = upsert(seen, for_the_harbor).await;
    assert_eq!(
        created["result"]["revision"],
        json!(seen + 1),
        "{created:#}"
    );
    // The client's new rule under that ID was made against the earlier
    // revision: it is a conflict, and the rule keeps its mobs.
    let stale = upsert(seen, new_rule.clone()).await;
    assert_eq!(
        stale["error"]["data"]["kind"],
        json!("access_revision_conflict"),
        "{stale:#}"
    );
    let (config, _) = access.snapshot();
    assert_eq!(config.rules.len(), 1);
    assert_eq!(config.rules[0].mobs, ["harbor"]);
    assert_eq!(
        config.rules[0].description.as_deref(),
        Some("for the harbor")
    );

    // Made against the current revision, the same write replaces the rule
    // whole: a client that creates a rule checks the ID itself.
    let replaced = upsert(seen + 1, new_rule).await;
    assert_eq!(
        replaced["result"]["revision"],
        json!(seen + 2),
        "{replaced:#}"
    );
    let (config, _) = access.snapshot();
    assert_eq!(config.rules.len(), 1);
    assert!(config.rules[0].mobs.is_empty());
    assert_eq!(config.rules[0].description.as_deref(), Some("a new rule"));
    Ok(())
}
