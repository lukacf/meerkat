//! An access file that carries a key no field reads is refused at load, and
//! the refusal names the file, the table and the key, every such key at
//! once. One row per effect a misspelt key had when it was left out: a rule
//! for every agent, a rule for every caller, a deny taken as an allow,
//! rules that are not enforced, and a file with no rule. A file of known
//! keys loads as before.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::access::{ACTION_AGENT_SEND, ACTION_AGENT_VIEW, AccessConfigError};

const ALICE: &str = "alice@example.test";
const CAROL: &str = "carol@example.test";

/// Ops may view every agent and send to the ops lead alone; nobody but an
/// admin touches the auditor.
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

[[rules]]
id = "protect-auditor"
effect = "deny"
actions = ["agent.*"]
agents = ["auditor"]
"#;

fn write_access_file(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("access.toml");
    std::fs::write(&path, text).expect("write the access file");
    (dir, path)
}

/// Load `text` as an access file. It must be refused as a file that could
/// not be parsed, with the file and each of `named` in the refusal. A file
/// that loads fails the row with what `loaded` reads from its controller.
fn assert_refused(text: &str, named: &[&str], loaded: impl FnOnce(&AccessController) -> String) {
    let (_dir, path) = write_access_file(text);
    let refusal = match AccessController::load(&path) {
        Err(AccessConfigError::Parse(refusal)) => refusal,
        Err(other) => panic!("refused as {other:?}"),
        Ok(controller) => panic!("the file loaded; {}", loaded(&controller)),
    };
    let file = path.display().to_string();
    assert!(refusal.contains(&file), "{file} in {refusal}");
    for text in named {
        assert!(refusal.contains(text), "{text} in {refusal}");
    }
    let shown = AccessConfigError::Parse(refusal).to_string();
    assert!(shown.contains(&file), "{file} in {shown}");
}

/// `agent` for `agents`: the rule would stand with no selector, for every
/// agent.
#[test]
fn a_rule_with_a_misspelt_selector_is_refused_not_loaded_for_every_agent() {
    assert_refused(
        &KNOWN_KEYS.replace("agents = [\"ops-lead\"]", "agent = [\"ops-lead\"]"),
        &["rule \"ops-send-lead\": unknown key \"agent\""],
        |controller| {
            format!(
                "ops may send to an agent the rule does not list: {}",
                controller
                    .view_for_subject(Some(ALICE))
                    .allows_agent(ACTION_AGENT_SEND, "scout")
            )
        },
    );
}

/// `group` for `groups` (and `subject` for `subjects`): the rule would
/// stand with no caller list, for every caller.
#[test]
fn a_rule_with_a_misspelt_caller_list_is_refused_not_loaded_for_every_caller() {
    for (written, misspelt, key) in [
        (
            "groups = [\"ops\"]\nactions = [\"agent.send\"]",
            "group = [\"ops\"]\nactions = [\"agent.send\"]",
            "group",
        ),
        (
            "groups = [\"ops\"]\nactions = [\"agent.send\"]",
            "subject = [\"alice@example.test\"]\nactions = [\"agent.send\"]",
            "subject",
        ),
    ] {
        let finding = format!("rule \"ops-send-lead\": unknown key \"{key}\"");
        assert_refused(
            &KNOWN_KEYS.replace(written, misspelt),
            &[finding.as_str()],
            |controller| {
                format!(
                    "a caller the rule does not list may send to the ops lead: {}",
                    controller
                        .view_for_subject(Some(CAROL))
                        .allows_agent(ACTION_AGENT_SEND, "ops-lead")
                )
            },
        );
    }
}

/// `efect` for `effect`: the deny would stand as an allow.
#[test]
fn a_rule_with_a_misspelt_effect_is_refused_not_loaded_as_an_allow() {
    assert_refused(
        &KNOWN_KEYS.replace("effect = \"deny\"", "efect = \"deny\""),
        &["rule \"protect-auditor\": unknown key \"efect\""],
        |controller| {
            format!(
                "a caller no other rule names may view the auditor: {}",
                controller
                    .view_for_subject(Some(CAROL))
                    .allows_agent(ACTION_AGENT_VIEW, "auditor")
            )
        },
    );
}

/// `enable` for `enabled`: the rules would not be enforced.
#[test]
fn a_file_with_a_misspelt_switch_is_refused_not_loaded_unenforced() {
    assert_refused(
        &KNOWN_KEYS.replace("enabled = true", "enable = true"),
        &["the configuration: unknown key \"enable\""],
        |controller| format!("the rules are enforced: {}", controller.enabled()),
    );
}

/// `[[rule]]` for `[[rules]]`: the file would stand with no rule.
#[test]
fn a_file_with_a_misspelt_rules_table_is_refused_not_loaded_without_rules() {
    assert_refused(
        &KNOWN_KEYS.replace("[[rules]]", "[[rule]]"),
        &["the configuration: unknown key \"rule\""],
        |controller| format!("rules loaded: {}", controller.snapshot().0.rules.len()),
    );
}

/// Every key no field reads is named in the one refusal: the
/// configuration's own, a group's and each rule's.
#[test]
fn every_stray_key_of_a_file_is_named_in_one_refusal() {
    assert_refused(
        r#"
enabled = true
admins = ["root@example.test"]
admin = ["carol@example.test"]

[groups.ops]
members = ["alice@example.test"]
member = ["bob@example.test"]

[[rules]]
id = "ops-view-all"
group = ["ops"]
actions = ["agent.view"]

[[rules]]
id = "protect-auditor"
efect = "deny"
actions = ["agent.*"]
agent = ["auditor"]
"#,
        &[
            "the configuration: unknown key \"admin\"",
            "group \"ops\": unknown key \"member\"",
            "rule \"ops-view-all\": unknown key \"group\"",
            "rule \"protect-auditor\": unknown key \"agent\"",
            "rule \"protect-auditor\": unknown key \"efect\"",
        ],
        |controller| format!("rules loaded: {}", controller.snapshot().0.rules.len()),
    );
}

/// A refused file names a key quoted, as it names the table: a quoted key
/// that holds a line break, a control character and a quote does not stand
/// raw in the refusal.
#[test]
fn a_refused_file_names_a_key_quoted() {
    let (_dir, path) = write_access_file(&KNOWN_KEYS.replace(
        "agents = [\"ops-lead\"]",
        "\"agent\\nadmins = [\\\"x\\\"]\\u001b[31m\" = [\"ops-lead\"]",
    ));
    let refusal = match AccessController::load(&path) {
        Err(AccessConfigError::Parse(refusal)) => refusal,
        Err(other) => panic!("refused as {other:?}"),
        Ok(_) => panic!("the file loaded"),
    };
    assert!(
        refusal.contains(
            "rule \"ops-send-lead\": unknown key \"agent\\nadmins = [\\\"x\\\"]\\u{1b}[31m\""
        ),
        "{refusal:?}"
    );
    assert!(!refusal.chars().any(char::is_control), "{refusal:?}");
}

/// Controls: a file of known keys loads and decides as written, and so does
/// the file the controller itself writes at a save.
#[test]
fn a_file_of_known_keys_loads_as_before() {
    let (_dir, path) = write_access_file(KNOWN_KEYS);
    let controller = AccessController::load(&path).expect("the file loads");
    let alice = controller.view_for_subject(Some(ALICE));
    assert!(alice.allows_agent(ACTION_AGENT_SEND, "ops-lead"));
    assert!(!alice.allows_agent(ACTION_AGENT_SEND, "scout"));
    assert!(alice.allows_agent(ACTION_AGENT_VIEW, "scout"));
    assert!(!alice.allows_agent(ACTION_AGENT_VIEW, "auditor"));
    let carol = controller.view_for_subject(Some(CAROL));
    assert!(!carol.allows_agent(ACTION_AGENT_SEND, "ops-lead"));

    controller.set_enabled(false).expect("a save");
    controller.set_enabled(true).expect("a save");
    let reloaded = AccessController::load(&path).expect("the written file loads");
    assert_eq!(reloaded.snapshot().0, controller.snapshot().0);
}

/// Control: an approver's person or ingress subject that carries a key no
/// field reads is refused at load, as before.
#[test]
fn an_approver_entry_with_a_stray_key_is_refused_as_before() {
    for entry in [
        "[[approvers.release.people]]\nsubjects = [\"carol@example.test\"]\nemail = \"carol\"\n",
        "[[approvers.release.ingress_subjects]]\ningress = \"operator\"\nname = \"carol\"\n",
    ] {
        let (_dir, path) = write_access_file(&format!("{KNOWN_KEYS}\n{entry}"));
        assert!(
            matches!(
                AccessController::load(&path),
                Err(AccessConfigError::Parse(_))
            ),
            "{entry}"
        );
    }
}
