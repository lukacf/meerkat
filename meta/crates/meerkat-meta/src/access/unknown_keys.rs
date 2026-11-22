//! Keys of an access configuration that no field of it reads.
//!
//! The configuration's own parse takes the keys its structs name. This walk
//! reads the same value before that parse and names every other key with the
//! place that holds it, all of them in one pass: the configuration itself, a
//! rule, a group, an approver, and an approver's people and ingress
//! subjects. A value of another shape than the structs expect is left to the
//! typed parse, which reports it.

use std::fmt;

use serde_json::{Map, Value};

/// The keys of [`super::model::AccessControlConfig`].
const CONFIG_KEYS: &[&str] = &["enabled", "admins", "groups", "rules", "approvers"];
/// The keys of [`super::model::AccessRule`].
const RULE_KEYS: &[&str] = &[
    "id",
    "description",
    "effect",
    "subjects",
    "groups",
    "actions",
    "agents",
    "roles",
    "match_labels",
    "mobs",
];
/// The keys of [`super::model::AccessGroup`].
const GROUP_KEYS: &[&str] = &["description", "members"];
/// The keys of [`super::model::AccessApprover`].
const APPROVER_KEYS: &[&str] = &["subjects", "groups", "ingress_subjects", "people"];
/// The keys of [`super::model::AccessApproverPerson`].
const APPROVER_PERSON_KEYS: &[&str] = &["subjects", "ingress_subjects"];
/// The keys of [`super::model::AccessIngressSubject`].
const INGRESS_SUBJECT_KEYS: &[&str] = &["ingress", "subject"];

/// The table of an access configuration a key was found in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AccessKeyPlace {
    /// The configuration's own table.
    Config,
    /// A rule: its `id` when it carries a text one, and its position in
    /// `rules` (from 1) when it was read as part of a configuration.
    Rule {
        id: Option<String>,
        position: Option<usize>,
    },
    /// A group, by its name.
    Group(String),
    /// An approver, by its id.
    Approver(String),
    /// An entry of an approver's `people` (position from 1).
    ApproverPerson { approver: String, position: usize },
    /// An entry of `ingress_subjects` (position from 1), of an approver or
    /// of one of its people.
    IngressSubject {
        approver: String,
        person: Option<usize>,
        position: usize,
    },
}

impl fmt::Display for AccessKeyPlace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config => write!(f, "the configuration"),
            Self::Rule {
                id: Some(id),
                position: _,
            } => write!(f, "rule {id:?}"),
            Self::Rule {
                id: None,
                position: Some(position),
            } => write!(f, "rule #{position} (no id)"),
            Self::Rule {
                id: None,
                position: None,
            } => write!(f, "the rule (no id)"),
            Self::Group(name) => write!(f, "group {name:?}"),
            Self::Approver(id) => write!(f, "approver {id:?}"),
            Self::ApproverPerson { approver, position } => {
                write!(f, "approver {approver:?}, person #{position}")
            }
            Self::IngressSubject {
                approver,
                person: None,
                position,
            } => write!(f, "approver {approver:?}, ingress subject #{position}"),
            Self::IngressSubject {
                approver,
                person: Some(person),
                position,
            } => write!(
                f,
                "approver {approver:?}, person #{person}, ingress subject #{position}"
            ),
        }
    }
}

/// One key no field reads, and the table it was written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnknownAccessKey {
    pub(crate) place: AccessKeyPlace,
    pub(crate) key: String,
}

impl fmt::Display for UnknownAccessKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: unknown key {:?}", self.place, self.key)
    }
}

/// Every finding, in the order found, as one line of text.
pub(crate) fn unknown_access_keys_text(keys: &[UnknownAccessKey]) -> String {
    let mut text = String::new();
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            text.push_str("; ");
        }
        text.push_str(&key.to_string());
    }
    text
}

/// The keys of a whole configuration that no field reads: its own, then its
/// groups' (by name), its rules' (in the order written) and its approvers'
/// (by id), the keys of one table in alphabetical order.
pub(crate) fn unknown_config_keys(config: &Value) -> Vec<UnknownAccessKey> {
    let mut found = Vec::new();
    let Some(root) = config.as_object() else {
        return found;
    };
    stray_keys(AccessKeyPlace::Config, root, CONFIG_KEYS, &mut found);
    for (name, group) in tables_by_name(root.get("groups")) {
        stray_keys(
            AccessKeyPlace::Group(name.to_string()),
            group,
            GROUP_KEYS,
            &mut found,
        );
    }
    for (position, rule) in tables_by_position(root.get("rules")) {
        rule_keys(rule, Some(position), &mut found);
    }
    for (id, approver) in tables_by_name(root.get("approvers")) {
        stray_keys(
            AccessKeyPlace::Approver(id.to_string()),
            approver,
            APPROVER_KEYS,
            &mut found,
        );
        ingress_subject_keys(id, None, approver.get("ingress_subjects"), &mut found);
        for (position, person) in tables_by_position(approver.get("people")) {
            stray_keys(
                AccessKeyPlace::ApproverPerson {
                    approver: id.to_string(),
                    position,
                },
                person,
                APPROVER_PERSON_KEYS,
                &mut found,
            );
            ingress_subject_keys(
                id,
                Some(position),
                person.get("ingress_subjects"),
                &mut found,
            );
        }
    }
    found
}

/// The keys of one rule that no field reads, as [`unknown_config_keys`]
/// finds them for a rule of a configuration.
pub(crate) fn unknown_rule_keys(rule: &Value) -> Vec<UnknownAccessKey> {
    let mut found = Vec::new();
    if let Some(rule) = rule.as_object() {
        rule_keys(rule, None, &mut found);
    }
    found
}

/// The keys of the group `name` that no field reads.
pub(crate) fn unknown_group_keys(name: &str, group: &Value) -> Vec<UnknownAccessKey> {
    let mut found = Vec::new();
    if let Some(group) = group.as_object() {
        stray_keys(
            AccessKeyPlace::Group(name.to_string()),
            group,
            GROUP_KEYS,
            &mut found,
        );
    }
    found
}

fn rule_keys(
    rule: &Map<String, Value>,
    position: Option<usize>,
    found: &mut Vec<UnknownAccessKey>,
) {
    let id = rule.get("id").and_then(Value::as_str).map(str::to_string);
    stray_keys(
        AccessKeyPlace::Rule { id, position },
        rule,
        RULE_KEYS,
        found,
    );
}

fn ingress_subject_keys(
    approver: &str,
    person: Option<usize>,
    entries: Option<&Value>,
    found: &mut Vec<UnknownAccessKey>,
) {
    for (position, entry) in tables_by_position(entries) {
        stray_keys(
            AccessKeyPlace::IngressSubject {
                approver: approver.to_string(),
                person,
                position,
            },
            entry,
            INGRESS_SUBJECT_KEYS,
            found,
        );
    }
}

/// The keys of `table` outside `known`, in alphabetical order.
fn stray_keys(
    place: AccessKeyPlace,
    table: &Map<String, Value>,
    known: &[&str],
    found: &mut Vec<UnknownAccessKey>,
) {
    let mut stray: Vec<&String> = table
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .collect();
    stray.sort();
    found.extend(stray.into_iter().map(|key| UnknownAccessKey {
        place: place.clone(),
        key: key.clone(),
    }));
}

/// The tables of a map of tables, by name in alphabetical order.
fn tables_by_name(value: Option<&Value>) -> Vec<(&str, &Map<String, Value>)> {
    let mut tables: Vec<(&str, &Map<String, Value>)> = value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, table)| Some((name.as_str(), table.as_object()?)))
        .collect();
    tables.sort_by_key(|(name, _)| *name);
    tables
}

/// The tables of a list of tables, with their positions from 1.
fn tables_by_position(value: Option<&Value>) -> impl Iterator<Item = (usize, &Map<String, Value>)> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, table)| Some((index + 1, table.as_object()?)))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::json;

    use super::*;
    use crate::access::model::{
        AccessApprover, AccessApproverPerson, AccessControlConfig, AccessEffect, AccessGroup,
        AccessIngressSubject, AccessRule,
    };

    fn toml_value(text: &str) -> Value {
        toml::from_str(text).expect("the text is TOML")
    }

    fn rule(id: &str, position: usize, key: &str) -> UnknownAccessKey {
        UnknownAccessKey {
            place: AccessKeyPlace::Rule {
                id: Some(id.to_string()),
                position: Some(position),
            },
            key: key.to_string(),
        }
    }

    fn at(place: AccessKeyPlace, key: &str) -> UnknownAccessKey {
        UnknownAccessKey {
            place,
            key: key.to_string(),
        }
    }

    const CLEAN: &str = r#"
enabled = true
admins = ["root@example.test"]

[groups.ops]
description = "Operations"
members = ["alice@example.test"]

[[rules]]
id = "ops-view-all"
description = "Ops see every agent"
groups = ["ops"]
actions = ["agent.view"]

[[rules]]
id = "ops-send-lead"
effect = "allow"
subjects = ["bob@example.test"]
actions = ["agent.send"]
agents = ["ops-lead"]
roles = ["lead"]
[rules.match_labels]
org = "payments"
tier = "one"

[approvers.release]
subjects = ["carol@example.test"]
groups = ["ops"]
[[approvers.release.ingress_subjects]]
ingress = "operator"
[[approvers.release.people]]
subjects = ["dave@example.test"]
[[approvers.release.people.ingress_subjects]]
ingress = "desk"
subject = "dave"
"#;

    /// A rule that carries a key no field reads is found, with the rule's id
    /// and the key.
    #[test]
    fn a_rule_with_a_stray_key_is_found_with_its_id_and_the_key() {
        let config = toml_value(
            r#"
enabled = true
admins = ["root@example.test"]

[[rules]]
id = "ops-view-all"
actions = ["agent.view"]

[[rules]]
id = "ops-send-lead"
actions = ["agent.send"]
agent = ["ops-lead"]
"#,
        );
        assert_eq!(
            unknown_config_keys(&config),
            vec![rule("ops-send-lead", 2, "agent")]
        );
    }

    /// Every such key of a configuration is found in one pass: the
    /// configuration's own, a group's, each rule's (a rule with no id by its
    /// position), an approver's, and those of an approver's people and
    /// ingress subjects.
    #[test]
    fn every_stray_key_of_a_configuration_is_found_in_one_pass() {
        let config = toml_value(
            r#"
enable = true
admins = ["root@example.test"]

[groups.ops]
member = ["alice@example.test"]

[[rules]]
id = "first"
efect = "deny"
subject = ["bob@example.test"]
actions = ["agent.view"]

[[rules]]
actions = ["agent.send"]
role = ["lead"]

[[rules]]
id = "third"
actions = ["agent.send"]
[rules.labels]
org = "payments"

[approvers.release]
subject = ["carol@example.test"]
[[approvers.release.ingress_subjects]]
ingress = "desk"
name = "carol"
[[approvers.release.people]]
email = "dave@example.test"
[[approvers.release.people.ingress_subjects]]
ingress = "desk"
subjects = ["dave"]
"#,
        );
        let approver = || "release".to_string();
        assert_eq!(
            unknown_config_keys(&config),
            vec![
                at(AccessKeyPlace::Config, "enable"),
                at(AccessKeyPlace::Group("ops".to_string()), "member"),
                rule("first", 1, "efect"),
                rule("first", 1, "subject"),
                at(
                    AccessKeyPlace::Rule {
                        id: None,
                        position: Some(2),
                    },
                    "role"
                ),
                rule("third", 3, "labels"),
                at(AccessKeyPlace::Approver(approver()), "subject"),
                at(
                    AccessKeyPlace::IngressSubject {
                        approver: approver(),
                        person: None,
                        position: 1,
                    },
                    "name"
                ),
                at(
                    AccessKeyPlace::ApproverPerson {
                        approver: approver(),
                        position: 1,
                    },
                    "email"
                ),
                at(
                    AccessKeyPlace::IngressSubject {
                        approver: approver(),
                        person: Some(1),
                        position: 1,
                    },
                    "subjects"
                ),
            ]
        );
    }

    /// A configuration that writes only keys its structs read finds nothing:
    /// as text, with label names of the host's own choosing, and as the
    /// structs themselves write it.
    #[test]
    fn a_clean_configuration_finds_nothing() {
        assert_eq!(unknown_config_keys(&toml_value(CLEAN)), Vec::new());
        let typed: AccessControlConfig = toml::from_str(CLEAN).expect("the text is a config");
        let written = serde_json::to_value(&typed).expect("the config is JSON");
        assert_eq!(unknown_config_keys(&written), Vec::new());
        let rewritten = toml::to_string_pretty(&typed).expect("the config is TOML");
        assert_eq!(unknown_config_keys(&toml_value(&rewritten)), Vec::new());
    }

    /// One rule and one group, as a save of either carries them, are read as
    /// they are inside a configuration.
    #[test]
    fn a_single_rule_and_a_single_group_are_read_the_same_way() {
        let place = AccessKeyPlace::Rule {
            id: Some("ops-send-lead".to_string()),
            position: None,
        };
        assert_eq!(
            unknown_rule_keys(&json!({
                "id": "ops-send-lead",
                "actions": ["agent.send"],
                "role": ["lead"],
                "agent": ["ops-lead"],
            })),
            vec![at(place.clone(), "agent"), at(place, "role")]
        );
        assert_eq!(
            unknown_rule_keys(&json!({
                "id": "ops-send-lead",
                "actions": ["agent.send"],
                "agents": ["ops-lead"],
                "match_labels": { "org": "payments" },
            })),
            Vec::new()
        );
        assert_eq!(
            unknown_group_keys("ops", &json!({ "member": ["alice@example.test"] })),
            vec![at(AccessKeyPlace::Group("ops".to_string()), "member")]
        );
        assert_eq!(
            unknown_group_keys(
                "ops",
                &json!({ "description": "Operations", "members": ["alice@example.test"] })
            ),
            Vec::new()
        );
    }

    /// A finding's text names its table and the key, and a list of findings
    /// is one line.
    #[test]
    fn a_finding_names_its_table_and_the_key() {
        let approver = || "release".to_string();
        let texts: Vec<String> = [
            at(AccessKeyPlace::Config, "enable"),
            rule("ops-send-lead", 2, "agent"),
            at(
                AccessKeyPlace::Rule {
                    id: None,
                    position: Some(3),
                },
                "role",
            ),
            at(
                AccessKeyPlace::Rule {
                    id: None,
                    position: None,
                },
                "role",
            ),
            at(AccessKeyPlace::Group("ops".to_string()), "member"),
            at(AccessKeyPlace::Approver(approver()), "subject"),
            at(
                AccessKeyPlace::ApproverPerson {
                    approver: approver(),
                    position: 2,
                },
                "email",
            ),
            at(
                AccessKeyPlace::IngressSubject {
                    approver: approver(),
                    person: None,
                    position: 1,
                },
                "name",
            ),
            at(
                AccessKeyPlace::IngressSubject {
                    approver: approver(),
                    person: Some(2),
                    position: 1,
                },
                "name",
            ),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            texts,
            [
                "the configuration: unknown key \"enable\"",
                "rule \"ops-send-lead\": unknown key \"agent\"",
                "rule #3 (no id): unknown key \"role\"",
                "the rule (no id): unknown key \"role\"",
                "group \"ops\": unknown key \"member\"",
                "approver \"release\": unknown key \"subject\"",
                "approver \"release\", person #2: unknown key \"email\"",
                "approver \"release\", ingress subject #1: unknown key \"name\"",
                "approver \"release\", person #2, ingress subject #1: unknown key \"name\"",
            ]
        );
        assert_eq!(
            unknown_access_keys_text(&[
                at(AccessKeyPlace::Config, "enable"),
                rule("ops-send-lead", 2, "agent"),
            ]),
            "the configuration: unknown key \"enable\"; rule \"ops-send-lead\": unknown key \"agent\""
        );
        assert_eq!(unknown_access_keys_text(&[]), "");
    }

    /// A key is written quoted, as the table names are: one that holds a
    /// line break, a control character or a quote does not stand raw in the
    /// text.
    #[test]
    fn a_key_is_written_quoted_as_the_table_names_are() {
        let found = unknown_rule_keys(&json!({
            "id": "ops-send-lead",
            "actions": ["agent.send"],
            "agent\nadmins = [\"x\"]\u{1b}[31m`": ["ops-lead"],
        }));
        let text = unknown_access_keys_text(&found);
        assert_eq!(
            text,
            "rule \"ops-send-lead\": unknown key \"agent\\nadmins = [\\\"x\\\"]\\u{1b}[31m`\""
        );
        assert!(!text.chars().any(char::is_control), "{text:?}");
    }

    /// A value of another shape than the structs expect is not walked: the
    /// typed parse reports it.
    #[test]
    fn values_of_another_shape_are_left_to_the_typed_parse() {
        for config in [
            json!("enabled"),
            json!({ "rules": "ops-send-lead", "groups": ["ops"], "approvers": 3 }),
            json!({ "rules": ["ops-send-lead", 3], "groups": { "ops": "alice" } }),
            json!({ "approvers": { "release": { "people": "dave", "ingress_subjects": [1] } } }),
        ] {
            assert_eq!(unknown_config_keys(&config), Vec::new(), "{config}");
        }
        assert_eq!(unknown_rule_keys(&json!(["agent"])), Vec::new());
        assert_eq!(unknown_group_keys("ops", &json!("member")), Vec::new());
    }

    fn keys_written(value: &impl serde::Serialize) -> BTreeSet<String> {
        serde_json::to_value(value)
            .expect("the value is JSON")
            .as_object()
            .expect("the value is a table")
            .keys()
            .cloned()
            .collect()
    }

    fn keys_known(known: &[&str]) -> BTreeSet<String> {
        known.iter().map(ToString::to_string).collect()
    }

    /// The keys the walk knows are the keys the structs write. Every struct
    /// is built here field by field, so a field added to one does not
    /// compile until it is written here, and then its key must be known.
    #[test]
    fn the_known_keys_are_the_keys_the_structs_write() {
        let text = || "x".to_string();
        let ingress_subject = AccessIngressSubject {
            ingress: text(),
            subject: Some(text()),
        };
        let person = AccessApproverPerson {
            subjects: vec![text()],
            ingress_subjects: vec![ingress_subject.clone()],
        };
        let approver = AccessApprover {
            subjects: vec![text()],
            groups: vec![text()],
            ingress_subjects: vec![ingress_subject.clone()],
            people: vec![person.clone()],
        };
        let group = AccessGroup {
            description: Some(text()),
            members: vec![text()],
        };
        let rule = AccessRule {
            id: text(),
            description: Some(text()),
            effect: AccessEffect::Deny,
            subjects: vec![text()],
            groups: vec![text()],
            actions: vec![text()],
            agents: vec![text()],
            roles: vec![text()],
            match_labels: BTreeMap::from([(text(), text())]),
            mobs: vec![text()],
        };
        let config = AccessControlConfig {
            enabled: true,
            admins: vec![text()],
            groups: BTreeMap::from([(text(), group.clone())]),
            rules: vec![rule.clone()],
            approvers: BTreeMap::from([(text(), approver.clone())]),
        };
        assert_eq!(keys_written(&config), keys_known(CONFIG_KEYS));
        assert_eq!(keys_written(&rule), keys_known(RULE_KEYS));
        assert_eq!(keys_written(&group), keys_known(GROUP_KEYS));
        assert_eq!(keys_written(&approver), keys_known(APPROVER_KEYS));
        assert_eq!(keys_written(&person), keys_known(APPROVER_PERSON_KEYS));
        assert_eq!(
            keys_written(&ingress_subject),
            keys_known(INGRESS_SUBJECT_KEYS)
        );
        assert_eq!(
            unknown_config_keys(&serde_json::to_value(&config).expect("JSON")),
            Vec::new()
        );
    }
}
