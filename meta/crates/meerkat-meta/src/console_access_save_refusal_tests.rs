//! A save through the Console's access methods whose configuration, rule
//! or group carries a key no field reads is refused: nothing is saved, and
//! the answer names every such key with its table. A save of known keys
//! applies as before.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::access::AccessControlConfig;

const ADMIN: &str = "root@example.test";

/// Ops may view every agent and send to the ops lead alone.
const KNOWN_KEYS: &str = r#"
enabled = true
admins = ["root@example.test"]

[groups.ops]
members = ["alice@example.test"]

[[rules]]
id = "ops-view-all"
groups = ["ops"]
actions = ["agent.view"]

[[rules]]
id = "ops-send-lead"
groups = ["ops"]
actions = ["agent.send"]
agents = ["ops-lead"]
"#;

fn console_controller() -> AccessController {
    let config: AccessControlConfig = toml::from_str(KNOWN_KEYS).expect("a configuration");
    AccessController::new(config).expect("controller")
}

/// One checked save through the Console's access methods, as an
/// administrator, against the controller's current revision.
fn save(controller: &AccessController, method: &str, mut payload: Value) -> Value {
    payload["owner_instance"] = json!(controller.edit_identity().expect("an owner instance"));
    payload["expected_revision"] = json!(controller.snapshot().1);
    let view = controller.view_for_subject(Some(ADMIN));
    handle_access_admin_rpc(
        Some(controller),
        Some(&view),
        &JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(1)),
            method: method.to_string(),
            params: json!({ "checked_v1": payload }),
        },
    )
    .expect("an access method")
}

/// A save whose configuration, rule or group carries a key no field reads
/// is refused as an invalid request that names the table and the key, every
/// one of them, and nothing is changed: no rule is saved without the
/// selector, the caller list or the effect its misspelt key was meant to
/// give it, and no configuration without its switch or its rules.
#[test]
fn a_save_with_a_stray_key_is_refused_with_table_and_key_and_changes_nothing() {
    let mut stray_config: Value = toml::from_str(KNOWN_KEYS).expect("a configuration");
    stray_config["enable"] = json!(true);
    stray_config["rule"] = json!([{ "id": "extra", "actions": ["agent.view"] }]);
    stray_config["rules"][1]["role"] = json!(["lead"]);
    stray_config["groups"]["ops"]["member"] = json!(["bob@example.test"]);
    for (method, payload, named) in [
        (
            "meta/access/rules/upsert",
            json!({ "rule": {
                "id": "ops-send-scout",
                "groups": ["ops"],
                "actions": ["agent.send"],
                "agent": ["scout"],
                "label": { "org": "payments" },
            }}),
            vec![
                "rule \"ops-send-scout\": unknown key \"agent\"",
                "rule \"ops-send-scout\": unknown key \"label\"",
            ],
        ),
        (
            "meta/access/rules/upsert",
            json!({ "rule": {
                "id": "protect-auditor",
                "efect": "deny",
                "subject": ["bob@example.test"],
                "group": ["ops"],
                "actions": ["agent.*"],
                "agents": ["auditor"],
            }}),
            vec![
                "rule \"protect-auditor\": unknown key \"efect\"",
                "rule \"protect-auditor\": unknown key \"group\"",
                "rule \"protect-auditor\": unknown key \"subject\"",
            ],
        ),
        (
            "meta/access/groups/set",
            json!({ "name": "contractors", "group": { "member": ["dave@example.test"] } }),
            vec!["group \"contractors\": unknown key \"member\""],
        ),
        (
            "meta/access/set",
            json!({ "config": stray_config }),
            vec![
                "the configuration: unknown key \"enable\"",
                "the configuration: unknown key \"rule\"",
                "group \"ops\": unknown key \"member\"",
                "rule \"ops-send-lead\": unknown key \"role\"",
            ],
        ),
    ] {
        let controller = console_controller();
        let before = controller.snapshot();
        let answer = save(&controller, method, payload);
        assert_eq!(answer["error"]["code"], json!(-32602), "{method}: {answer}");
        assert_eq!(answer["result"], Value::Null, "{method}: {answer}");
        let message = answer["error"]["message"].as_str().unwrap_or_default();
        for text in named {
            assert!(message.contains(text), "{method}: {text} in {message}");
        }
        assert_eq!(
            controller.snapshot(),
            before,
            "{method}: nothing is changed"
        );
    }
}

/// A refused save names a key quoted, as it names the table: a key that
/// holds a line break, a control character or a quote does not stand raw in
/// the answer.
#[test]
fn a_refused_save_names_a_key_quoted() {
    let controller = console_controller();
    let answer = save(
        &controller,
        "meta/access/rules/upsert",
        json!({ "rule": {
            "id": "ops-send-scout",
            "actions": ["agent.send"],
            "agent\nadmins = [\"x\"]\u{1b}[31m": ["scout"],
        }}),
    );
    assert_eq!(answer["error"]["code"], json!(-32602), "{answer}");
    let message = answer["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(
            "rule \"ops-send-scout\": unknown key \"agent\\nadmins = [\\\"x\\\"]\\u{1b}[31m\""
        ),
        "{message:?}"
    );
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert_eq!(controller.snapshot().1, 0, "nothing is changed");
}

/// Control: the same saves with known keys apply as before.
#[test]
fn a_save_of_known_keys_applies_as_before() {
    let controller = console_controller();
    let upsert = save(
        &controller,
        "meta/access/rules/upsert",
        json!({ "rule": {
            "id": "ops-send-scout",
            "description": "Ops reach the scout",
            "effect": "allow",
            "subjects": ["bob@example.test"],
            "groups": ["ops"],
            "actions": ["agent.send"],
            "agents": ["scout"],
            "roles": ["scout"],
            "match_labels": { "org": "payments" },
        }}),
    );
    assert_eq!(upsert["result"]["revision"], json!(1), "{upsert}");
    let group = save(
        &controller,
        "meta/access/groups/set",
        json!({ "name": "contractors", "group": {
            "description": "Contractors",
            "members": ["dave@example.test"],
        }}),
    );
    assert_eq!(group["result"]["revision"], json!(2), "{group}");
    let mut whole = serde_json::to_value(&*controller.snapshot().0).expect("a configuration");
    whole["admins"] = json!([ADMIN, "carol@example.test"]);
    let set = save(&controller, "meta/access/set", json!({ "config": whole }));
    assert_eq!(set["result"]["revision"], json!(3), "{set}");
    let (config, _) = controller.snapshot();
    let rule = config
        .rules
        .iter()
        .find(|rule| rule.id == "ops-send-scout")
        .expect("the saved rule");
    assert_eq!(rule.agents, ["scout"]);
    assert_eq!(config.groups["contractors"].members, ["dave@example.test"]);
    assert_eq!(config.admins.len(), 2);
}

/// Control: a checked save that carries no configuration, rule or group,
/// and a request that is no checked save, are answered as before.
#[test]
fn requests_that_carry_no_table_are_answered_as_before() {
    let controller = console_controller();
    let deleted = save(
        &controller,
        "meta/access/rules/delete",
        json!({ "id": "ops-send-lead" }),
    );
    assert_eq!(deleted["result"]["revision"], json!(1), "{deleted}");
    let disabled = save(
        &controller,
        "meta/access/enable",
        json!({ "enabled": false }),
    );
    assert_eq!(disabled["result"]["revision"], json!(2), "{disabled}");
    let controller = console_controller();
    let view = controller.view_for_subject(Some(ADMIN));
    let unchecked = handle_access_admin_rpc(
        Some(&controller),
        Some(&view),
        &JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(json!(1)),
            method: "meta/access/rules/upsert".to_string(),
            params: json!({ "rule": { "id": "x", "actions": ["agent.view"], "agent": ["y"] } }),
        },
    )
    .expect("an access method");
    assert_eq!(
        unchecked["error"]["data"]["kind"],
        json!("revision_required"),
        "{unchecked}"
    );
}
