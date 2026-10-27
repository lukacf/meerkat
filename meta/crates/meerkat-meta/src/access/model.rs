//! Access-control configuration schema and validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// View an agent in any console surface: sidebar, roster, topology,
/// timeline frames, identity status, inspection, and event streams.
pub const ACTION_AGENT_VIEW: &str = "agent.view";
/// Send a message to an agent (console send / chat).
pub const ACTION_AGENT_SEND: &str = "agent.send";
/// Write durable identity-scoped memory records for future agent context.
pub const ACTION_AGENT_MEMORY_WRITE: &str = "agent.memory.write";
/// Delete durable identity-scoped memory records.
pub const ACTION_AGENT_MEMORY_DELETE: &str = "agent.memory.delete";
/// Read identity-scoped memory: the recall/manifest RPCs and every console
/// Memory-panel read (§10.3). Realm-scoped reads ride an *unscoped*
/// `agent.memory.read` grant (a rule with no resource selector).
pub const ACTION_AGENT_MEMORY_READ: &str = "agent.memory.read";
/// Read operator-scoped memory records (§10.3): cross-mob personal facts
/// about the operator, more sensitive than any other scope. Never implied
/// by an unscoped `agent.memory.read` grant and never granted by the
/// migration compat rewrite — always an explicit rule.
pub const ACTION_OPERATOR_MEMORY_READ: &str = "operator.memory.read";
/// Administrative memory operations on an identity's store (imports,
/// re-keying, floor overrides). Reserved: no console RPC maps to it yet.
pub const ACTION_AGENT_MEMORY_ADMIN: &str = "agent.memory.admin";
/// Read mob-scoped memory records in the console Memory panel.
pub const ACTION_MOB_MEMORY_READ: &str = "mob.memory.read";
/// Propose a record into mob scope (`propose` surfaces).
pub const ACTION_MOB_MEMORY_PROPOSE: &str = "mob.memory.propose";
/// Commit records directly into mob scope. Reserved for a future direct
/// commit RPC — steward promotions ride the gating flow (`gating.decide`),
/// not this action, so nothing maps to it yet.
pub const ACTION_MOB_MEMORY_COMMIT: &str = "mob.memory.commit";
/// Read the quarantine queue and its verdict surfaces (§10.3).
pub const ACTION_MEMORY_QUARANTINE_REVIEW: &str = "memory.quarantine.review";
/// Create new members: ensure/spawn/fork helpers, run flows.
pub const ACTION_AGENT_SPAWN: &str = "agent.spawn";
/// Respawn an existing agent.
pub const ACTION_AGENT_RESPAWN: &str = "agent.respawn";
/// Retire / force-cancel / delete an agent.
pub const ACTION_AGENT_RETIRE: &str = "agent.retire";
/// Reset an agent's durable state.
pub const ACTION_AGENT_RESET: &str = "agent.reset";
/// Read gating queues and audit history.
pub const ACTION_GATING_VIEW: &str = "gating.view";
/// Decide pending gating approvals.
pub const ACTION_GATING_DECIDE: &str = "gating.decide";
/// Read WorkGraph state: snapshots, items, attention bindings, events.
pub const ACTION_WORKGRAPH_VIEW: &str = "workgraph.view";
/// Mutate WorkGraph state: goals, item lifecycle, attention operations.
pub const ACTION_WORKGRAPH_MANAGE: &str = "workgraph.manage";
/// Subscribe to whole-mob event surfaces (raw mob/structural event streams).
pub const ACTION_MOB_OBSERVE: &str = "mob.observe";
/// Operate runtime plumbing: routing tables, labels, wiring, reconcile.
pub const ACTION_RUNTIME_ADMIN: &str = "runtime.admin";
/// Inspect topology for an agent. Query results require this and
/// `agent.view` on every visible endpoint.
pub const ACTION_TOPOLOGY_VIEW: &str = "topology.view";
/// Read durable topology mutation audit records, including actor/principal
/// attribution. Kept separate from topology.view because operator identity is
/// more sensitive than graph shape.
pub const ACTION_TOPOLOGY_AUDIT: &str = "topology.audit";
/// Add a desired peer edge. Evaluated independently against both endpoints.
pub const ACTION_TOPOLOGY_CONNECT: &str = "topology.connect";
/// Suppress and remove a peer edge. Evaluated against both endpoints.
pub const ACTION_TOPOLOGY_DISCONNECT: &str = "topology.disconnect";
/// Remove a suppression and repair the desired edge. Kept distinct from
/// connect so deployments can gate restoration more tightly.
pub const ACTION_TOPOLOGY_RECONNECT: &str = "topology.reconnect";
/// Submit more than one explicit topology operation in one transaction.
pub const ACTION_TOPOLOGY_BULK: &str = "topology.bulk";
/// Mutate an edge spanning two authorities through the explicit same-process
/// bilateral host coordinator. Authority-local RPC never advertises or
/// accepts this operation.
pub const ACTION_TOPOLOGY_CROSS_AUTHORITY: &str = "topology.cross_authority";
/// Author mobpacks in the Flow Editor: drafts, authoring operations,
/// validation, source rendering, export/import, and authoring catalogs.
pub const ACTION_MOBPACK_AUTHOR: &str = "mobpack.author";
/// Execute a mobpack deploy on the host (`rkat mob run`).
pub const ACTION_MOBPACK_DEPLOY: &str = "mobpack.deploy";
/// Read and mutate the access-control configuration itself.
pub const ACTION_ACCESS_ADMIN: &str = "access.admin";

/// The full action vocabulary, in display order.
pub const ACCESS_ACTIONS: &[&str] = &[
    #[cfg(feature = "extension-state")]
    "extension.read",
    #[cfg(feature = "extension-state")]
    "extension.edit",
    #[cfg(feature = "extension-state")]
    "extension.manage",
    ACTION_AGENT_VIEW,
    ACTION_AGENT_SEND,
    ACTION_AGENT_MEMORY_WRITE,
    ACTION_AGENT_MEMORY_DELETE,
    ACTION_AGENT_MEMORY_READ,
    ACTION_AGENT_MEMORY_ADMIN,
    ACTION_OPERATOR_MEMORY_READ,
    ACTION_MOB_MEMORY_READ,
    ACTION_MOB_MEMORY_PROPOSE,
    ACTION_MOB_MEMORY_COMMIT,
    ACTION_MEMORY_QUARANTINE_REVIEW,
    ACTION_AGENT_SPAWN,
    ACTION_AGENT_RESPAWN,
    ACTION_AGENT_RETIRE,
    ACTION_AGENT_RESET,
    ACTION_GATING_VIEW,
    ACTION_GATING_DECIDE,
    ACTION_WORKGRAPH_VIEW,
    ACTION_WORKGRAPH_MANAGE,
    ACTION_MOB_OBSERVE,
    ACTION_RUNTIME_ADMIN,
    ACTION_TOPOLOGY_VIEW,
    ACTION_TOPOLOGY_AUDIT,
    ACTION_TOPOLOGY_CONNECT,
    ACTION_TOPOLOGY_DISCONNECT,
    ACTION_TOPOLOGY_RECONNECT,
    ACTION_TOPOLOGY_BULK,
    ACTION_TOPOLOGY_CROSS_AUTHORITY,
    ACTION_MOBPACK_AUTHOR,
    ACTION_MOBPACK_DEPLOY,
    ACTION_ACCESS_ADMIN,
];

/// Root access-control configuration. Serializable as TOML
/// (`config/access.toml`) and JSON (RPC admin surface).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessControlConfig {
    /// Master switch. When `false` the controller is a transparent no-op
    /// and every surface behaves exactly as if no access control existed.
    #[serde(default)]
    pub enabled: bool,
    /// Subjects with unconditional full access, including the right to
    /// edit this configuration. Must be non-empty while `enabled` is true
    /// so a bad rule set can never lock every administrator out.
    #[serde(default)]
    pub admins: Vec<String>,
    /// Named groups of subjects. Group membership is the per-user live
    /// configuration surface: assigning a subject to a group immediately
    /// changes what every rule referencing that group grants them.
    #[serde(default)]
    pub groups: BTreeMap<String, AccessGroup>,
    /// Attribute rules, evaluated as a set (order is irrelevant;
    /// deny-overrides-allow).
    #[serde(default)]
    pub rules: Vec<AccessRule>,
    /// Approver identities and who holds them. Keys are the approver ids a
    /// gating request can name in `requested_approver` (for example
    /// `person:alice` or `role:release-manager`). Holders are read from the
    /// live configuration when a decision is made, so removing a holder, or
    /// a subject from a holding group, revokes the identity immediately.
    /// Several principals may hold one id (a shared role).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approvers: BTreeMap<String, AccessApprover>,
}

/// Who holds one approver identity: listed subjects and members of listed
/// groups.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessApprover {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
}

/// A named set of subjects.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub members: Vec<String>,
}

/// Allow or deny.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessEffect {
    #[default]
    Allow,
    Deny,
}

/// One attribute rule.
///
/// Dimension semantics:
/// - `subjects` / `groups`: the rule applies to a principal when its
///   subject is listed in `subjects` (or `subjects` contains `"*"`), or it
///   belongs to any listed group. When both lists are empty the rule
///   applies to every principal, including unauthenticated ones.
/// - `actions`: required, non-empty. Entries are exact action names,
///   `"prefix.*"` wildcards, or `"*"`.
/// - `agents` / `roles` / `match_labels`: resource selectors. Each
///   specified selector must match (logical AND across dimensions); within
///   `agents` and `roles` any listed value matches (logical OR), and
///   `"*"` matches every value. Empty selectors leave that dimension
///   unconstrained. A rule with all three
///   empty matches every resource, including action checks that have no
///   resource at all (e.g. `gating.decide`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRule {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub effect: AccessEffect,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub match_labels: BTreeMap<String, String>,
}

impl AccessRule {
    /// True when the rule constrains the resource in any way.
    pub fn has_resource_selector(&self) -> bool {
        !self.agents.is_empty() || !self.roles.is_empty() || !self.match_labels.is_empty()
    }
}

/// Validation failure for an [`AccessControlConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessConfigError {
    EnabledWithoutAdmins,
    EmptyRuleId,
    DuplicateRuleId(String),
    EmptyActions(String),
    UnknownAction { rule: String, action: String },
    UnknownGroup { rule: String, group: String },
    EmptyApproverId,
    ApproverWithoutHolders(String),
    UnknownApproverGroup { approver: String, group: String },
    UnknownRule(String),
    Parse(String),
    Io(String),
    RevisionExhausted,
}

impl std::fmt::Display for AccessConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnabledWithoutAdmins => write!(
                f,
                "access control cannot be enabled without at least one admin subject"
            ),
            Self::EmptyRuleId => write!(f, "access rule id must not be empty"),
            Self::EmptyApproverId => write!(f, "approver id must not be empty"),
            Self::ApproverWithoutHolders(id) => {
                write!(
                    f,
                    "approver {id}: at least one subject or group must hold it"
                )
            }
            Self::UnknownApproverGroup { approver, group } => {
                write!(f, "approver {approver}: unknown group {group:?}")
            }
            Self::DuplicateRuleId(id) => write!(f, "duplicate access rule id: {id}"),
            Self::EmptyActions(id) => write!(f, "access rule {id}: actions must not be empty"),
            Self::UnknownAction { rule, action } => {
                write!(f, "access rule {rule}: unknown action {action:?}")
            }
            Self::UnknownGroup { rule, group } => {
                write!(f, "access rule {rule}: unknown group {group:?}")
            }
            Self::UnknownRule(id) => write!(f, "unknown access rule id: {id}"),
            Self::Parse(message) => write!(f, "access config could not be parsed: {message}"),
            Self::Io(message) => write!(f, "access config io error: {message}"),
            Self::RevisionExhausted => write!(f, "access configuration revision exhausted"),
        }
    }
}

impl std::error::Error for AccessConfigError {}

fn action_pattern_is_known(pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(".*") {
        return ACCESS_ACTIONS.iter().any(|action| {
            action
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.'))
        });
    }
    ACCESS_ACTIONS.contains(&pattern)
}

/// Whether a rule action pattern (`*`, `prefix.*`, or an exact name)
/// matches a concrete action. Single source of truth for pattern
/// semantics, shared with rule evaluation.
pub(crate) fn action_pattern_matches(pattern: &str, action: &str) -> bool {
    if pattern == "*" || pattern == action {
        return true;
    }
    pattern.strip_suffix(".*").is_some_and(|prefix| {
        action
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// True when the pattern explicitly references the memory action family —
/// one of its dot-separated segments (with a trailing `.*` stripped) is
/// `memory`. Broad wildcards (`*`, `agent.*`) do NOT count: they already
/// match the memory actions through ordinary pattern semantics and need no
/// compat handling.
fn pattern_mentions_memory(pattern: &str) -> bool {
    pattern
        .strip_suffix(".*")
        .unwrap_or(pattern)
        .split('.')
        .any(|segment| segment == "memory")
}

/// §10.3 migration compat: configs written before the per-scope memory read
/// actions existed keep working.
///
/// A config that mentions **no** memory action in any rule (neither the
/// pre-existing `agent.memory.write`/`delete` nor any of the new read
/// actions) is treated as memory-naive: every rule matching `agent.view`
/// is extended to also cover `agent.memory.read`, for allow *and* deny
/// rules alike, so "read rides view" exactly reproduces the pre-migration
/// recall behavior (deny-overrides included). A config that mentions any
/// memory action anywhere is taken literally and left untouched.
///
/// The extension is materialized into the rule list (and therefore into the
/// persisted TOML on the next admin save), which also makes the rewrite
/// self-limiting: a normalized config mentions `agent.memory.read` and is
/// never rewritten again. Returns `true` when anything changed; callers log
/// the recommendation to write explicit memory rules.
pub fn normalize_access_config_for_memory_actions(config: &mut AccessControlConfig) -> bool {
    let mentions_memory = config
        .rules
        .iter()
        .flat_map(|rule| rule.actions.iter())
        .any(|pattern| pattern_mentions_memory(pattern));
    if mentions_memory {
        return false;
    }
    let mut changed = false;
    for rule in &mut config.rules {
        let matches_view = rule
            .actions
            .iter()
            .any(|pattern| action_pattern_matches(pattern, ACTION_AGENT_VIEW));
        let matches_read = rule
            .actions
            .iter()
            .any(|pattern| action_pattern_matches(pattern, ACTION_AGENT_MEMORY_READ));
        if matches_view && !matches_read {
            rule.actions.push(ACTION_AGENT_MEMORY_READ.to_string());
            changed = true;
        }
    }
    if changed {
        tracing::warn!(
            target: "meta::access",
            "access config predates memory read actions; granting agent.memory.read wherever \
             agent.view is granted (write explicit agent.memory.read / mob.memory.read / \
             memory.quarantine.review rules to silence this)"
        );
    }
    changed
}

/// Validate a configuration before accepting it.
///
/// Enforces the anti-lockout invariant (enabled implies admins), unique
/// non-empty rule ids, a non-empty known action list per rule, and that
/// every referenced group is defined.
pub fn validate_access_config(config: &AccessControlConfig) -> Result<(), AccessConfigError> {
    if config.enabled && config.admins.iter().all(|admin| admin.trim().is_empty()) {
        return Err(AccessConfigError::EnabledWithoutAdmins);
    }
    let mut seen_ids = std::collections::BTreeSet::new();
    for rule in &config.rules {
        if rule.id.trim().is_empty() {
            return Err(AccessConfigError::EmptyRuleId);
        }
        if !seen_ids.insert(rule.id.as_str()) {
            return Err(AccessConfigError::DuplicateRuleId(rule.id.clone()));
        }
        if rule.actions.is_empty() {
            return Err(AccessConfigError::EmptyActions(rule.id.clone()));
        }
        for action in &rule.actions {
            if !action_pattern_is_known(action) {
                return Err(AccessConfigError::UnknownAction {
                    rule: rule.id.clone(),
                    action: action.clone(),
                });
            }
        }
        for group in &rule.groups {
            if !config.groups.contains_key(group) {
                return Err(AccessConfigError::UnknownGroup {
                    rule: rule.id.clone(),
                    group: group.clone(),
                });
            }
        }
    }
    for (approver, holders) in &config.approvers {
        if approver.trim().is_empty() || approver.trim() != approver {
            return Err(AccessConfigError::EmptyApproverId);
        }
        if holders
            .subjects
            .iter()
            .all(|subject| subject.trim().is_empty())
            && holders.groups.is_empty()
        {
            return Err(AccessConfigError::ApproverWithoutHolders(approver.clone()));
        }
        for group in &holders.groups {
            if !config.groups.contains_key(group) {
                return Err(AccessConfigError::UnknownApproverGroup {
                    approver: approver.clone(),
                    group: group.clone(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rule(id: &str, actions: &[&str]) -> AccessRule {
        AccessRule {
            id: id.to_string(),
            actions: actions.iter().map(ToString::to_string).collect(),
            ..AccessRule::default()
        }
    }

    #[test]
    fn default_config_is_disabled_and_valid() {
        let config = AccessControlConfig::default();
        assert!(!config.enabled);
        assert!(validate_access_config(&config).is_ok());
    }

    #[test]
    fn enabling_requires_admins() {
        let config = AccessControlConfig {
            enabled: true,
            ..AccessControlConfig::default()
        };
        assert_eq!(
            validate_access_config(&config),
            Err(AccessConfigError::EnabledWithoutAdmins)
        );
    }

    #[test]
    fn rules_require_known_actions() {
        let mut config = AccessControlConfig {
            admins: vec!["root@example.test".to_string()],
            rules: vec![rule(
                "r1",
                &["agent.view", "agent.memory.*", "agent.*", "*"],
            )],
            ..AccessControlConfig::default()
        };
        assert!(validate_access_config(&config).is_ok());
        config.rules.push(rule("r2", &["agent.fly"]));
        assert!(matches!(
            validate_access_config(&config),
            Err(AccessConfigError::UnknownAction { .. })
        ));
    }

    #[test]
    fn rules_reject_duplicate_ids_and_unknown_groups() {
        let mut config = AccessControlConfig {
            rules: vec![rule("r1", &["agent.view"]), rule("r1", &["agent.send"])],
            ..AccessControlConfig::default()
        };
        assert_eq!(
            validate_access_config(&config),
            Err(AccessConfigError::DuplicateRuleId("r1".to_string()))
        );
        config.rules.pop();
        config.rules[0].groups = vec!["ops".to_string()];
        assert!(matches!(
            validate_access_config(&config),
            Err(AccessConfigError::UnknownGroup { .. })
        ));
    }

    #[test]
    fn memory_actions_validate() {
        let config = AccessControlConfig {
            admins: vec!["root@example.test".to_string()],
            rules: vec![rule(
                "r1",
                &[
                    "agent.memory.read",
                    "agent.memory.admin",
                    "operator.memory.read",
                    "mob.memory.read",
                    "mob.memory.propose",
                    "mob.memory.commit",
                    "memory.quarantine.review",
                    "mob.memory.*",
                    "memory.*",
                ],
            )],
            ..AccessControlConfig::default()
        };
        assert!(validate_access_config(&config).is_ok());
    }

    #[test]
    fn memory_naive_config_grants_read_alongside_view() {
        // Pre-migration config: view granted broadly, view denied on one
        // agent, an unrelated send rule. No memory action anywhere.
        let mut deny_view = rule("deny-secret", &["agent.view"]);
        deny_view.effect = AccessEffect::Deny;
        deny_view.agents = vec!["identity:secret".to_string()];
        let mut config = AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                rule("view-all", &["agent.view"]),
                deny_view,
                rule("send-one", &["agent.send"]),
            ],
            ..AccessControlConfig::default()
        };
        assert!(normalize_access_config_for_memory_actions(&mut config));
        let actions_of = |id: &str| {
            config
                .rules
                .iter()
                .find(|rule| rule.id == id)
                .expect("rule")
                .actions
                .clone()
        };
        assert!(actions_of("view-all").contains(&"agent.memory.read".to_string()));
        assert!(
            actions_of("deny-secret").contains(&"agent.memory.read".to_string()),
            "denies mirror too, so read cannot outlive a view deny"
        );
        assert!(!actions_of("send-one").contains(&"agent.memory.read".to_string()));
        // Idempotent: the normalized config now mentions memory.
        assert!(!normalize_access_config_for_memory_actions(&mut config));
    }

    #[test]
    fn config_mentioning_any_memory_action_is_taken_literally() {
        // The pre-existing write action counts as "mentions memory": the
        // author knew about memory actions, so the absence of read rules is
        // an explicit choice.
        let mut config = AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![
                rule("view-all", &["agent.view"]),
                rule("writer", &["agent.memory.write"]),
            ],
            ..AccessControlConfig::default()
        };
        assert!(!normalize_access_config_for_memory_actions(&mut config));
        assert!(
            !config.rules[0]
                .actions
                .contains(&"agent.memory.read".to_string())
        );

        // A prefix wildcard naming the family counts as a mention as well.
        let mut config = AccessControlConfig {
            rules: vec![
                rule("view-all", &["agent.view"]),
                rule("mem", &["agent.memory.*"]),
            ],
            ..AccessControlConfig::default()
        };
        assert!(!normalize_access_config_for_memory_actions(&mut config));
    }

    #[test]
    fn broad_wildcards_do_not_trigger_or_need_compat() {
        // `agent.*` already matches agent.memory.read through pattern
        // semantics, so the rule needs no rewrite; `*` likewise.
        let mut config = AccessControlConfig {
            rules: vec![rule("all-agent-verbs", &["agent.*"])],
            ..AccessControlConfig::default()
        };
        assert!(!normalize_access_config_for_memory_actions(&mut config));
        assert!(action_pattern_matches("agent.*", "agent.memory.read"));
        assert!(action_pattern_matches("*", "memory.quarantine.review"));
        assert!(!action_pattern_matches("agent.*", "mob.memory.read"));
    }

    #[test]
    fn config_round_trips_through_toml() {
        let config = AccessControlConfig {
            enabled: true,
            approvers: Default::default(),
            admins: vec!["root@example.test".to_string()],
            groups: BTreeMap::from([(
                "ops".to_string(),
                AccessGroup {
                    description: Some("Operations".to_string()),
                    members: vec!["alice@example.test".to_string()],
                },
            )]),
            rules: vec![AccessRule {
                id: "ops-see-all".to_string(),
                description: Some("ops see everything".to_string()),
                effect: AccessEffect::Allow,
                groups: vec!["ops".to_string()],
                actions: vec!["agent.view".to_string()],
                ..AccessRule::default()
            }],
        };
        let toml = toml::to_string_pretty(&config).expect("serialize");
        let parsed: AccessControlConfig = toml::from_str(&toml).expect("parse");
        assert_eq!(parsed, config);
    }

    #[test]
    fn approver_bindings_validate_ids_holders_and_groups() {
        let with = |id: &str, holders: AccessApprover| AccessControlConfig {
            approvers: BTreeMap::from([(id.to_string(), holders)]),
            ..AccessControlConfig::default()
        };
        let subject = |name: &str| AccessApprover {
            subjects: vec![name.to_string()],
            ..AccessApprover::default()
        };
        assert_eq!(
            validate_access_config(&with("person:alice", subject("alice@x"))),
            Ok(())
        );
        assert_eq!(
            validate_access_config(&with("", subject("alice@x"))),
            Err(AccessConfigError::EmptyApproverId)
        );
        assert_eq!(
            validate_access_config(&with(" person:alice", subject("alice@x"))),
            Err(AccessConfigError::EmptyApproverId)
        );
        assert_eq!(
            validate_access_config(&with("person:alice", AccessApprover::default())),
            Err(AccessConfigError::ApproverWithoutHolders(
                "person:alice".to_string()
            ))
        );
        assert_eq!(
            validate_access_config(&with(
                "role:release",
                AccessApprover {
                    groups: vec!["missing".to_string()],
                    ..AccessApprover::default()
                }
            )),
            Err(AccessConfigError::UnknownApproverGroup {
                approver: "role:release".to_string(),
                group: "missing".to_string(),
            })
        );
        let toml_text = "[approvers.\"role:release\"]\nsubjects = [\"carol@x\"]\n";
        let parsed: AccessControlConfig = toml::from_str(toml_text).expect("parse");
        assert_eq!(parsed.approvers["role:release"].subjects, vec!["carol@x"]);
    }
}
