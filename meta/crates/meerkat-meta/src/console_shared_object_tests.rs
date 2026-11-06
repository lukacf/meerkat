//! Shared console objects one caller cannot quietly take from another: an
//! access edit is written only against the revision its editor read, and on
//! a console without an access controller a caller revokes only their own
//! sessions.

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
