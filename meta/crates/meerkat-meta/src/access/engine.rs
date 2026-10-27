//! Pure ABAC evaluation. No locks, no IO — config in, decision out.

use std::collections::{BTreeMap, BTreeSet};

use super::model::{AccessControlConfig, AccessEffect, AccessRule, action_pattern_matches};

/// The authenticated caller's attributes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessPrincipal {
    /// Authenticated subject (email or token `sub`). `None` means the
    /// request was admitted without app auth (open console); such callers
    /// only match rules with no subject/group constraints.
    pub subject: Option<String>,
    /// Resolved group memberships for the subject.
    pub groups: BTreeSet<String>,
}

impl AccessPrincipal {
    pub fn anonymous() -> Self {
        Self::default()
    }
}

/// Attributes of the resource a check targets. All fields are optional:
/// checks for non-agent actions (e.g. `gating.decide`) carry no resource
/// at all and only match rules without resource selectors.
#[derive(Debug, Clone, Copy, Default)]
pub struct AccessResource<'a> {
    /// Agent identity (preferred key, e.g. `identity:ops-lead`).
    pub identity: Option<&'a str>,
    /// Runtime agent/member id, matched against rule `agents` as a fallback.
    pub agent_id: Option<&'a str>,
    /// Agent role/profile name.
    pub role: Option<&'a str>,
    /// Agent labels. `None` means the attributes are unknown (label
    /// selectors cannot match); `Some` empty means known-empty.
    pub labels: Option<&'a BTreeMap<String, String>>,
}

impl<'a> AccessResource<'a> {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn for_identity(identity: &'a str) -> Self {
        Self {
            identity: Some(identity),
            ..Self::default()
        }
    }

    fn is_present(&self) -> bool {
        self.identity.is_some() || self.agent_id.is_some()
    }
}

/// Outcome of one access check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    Deny { reason: String },
}

impl AccessDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allow => None,
            Self::Deny { reason } => Some(reason),
        }
    }
}

fn action_matches(pattern: &str, action: &str) -> bool {
    action_pattern_matches(pattern, action)
}

fn subject_matches(rule: &AccessRule, principal: &AccessPrincipal) -> bool {
    if rule.subjects.is_empty() && rule.groups.is_empty() {
        return true;
    }
    // `subjects: ["*"]` matches any *authenticated* subject. It does NOT
    // match the anonymous principal: an anonymous caller only matches rules
    // with no subject/group constraints at all (see `AccessPrincipal.subject`
    // and the resource selectors, where `"*"` likewise never matches an
    // absent attribute). This keeps `subjects:["*"]` from silently granting
    // unauthenticated callers on an open console.
    if let Some(subject) = principal.subject.as_deref()
        && rule
            .subjects
            .iter()
            .any(|candidate| candidate == "*" || candidate == subject)
    {
        return true;
    }
    rule.groups
        .iter()
        .any(|group| principal.groups.contains(group))
}

fn resource_matches(rule: &AccessRule, resource: &AccessResource<'_>) -> bool {
    if !rule.has_resource_selector() {
        return true;
    }
    // A constrained rule can never match a check that has no resource.
    if !resource.is_present() {
        return false;
    }
    if !rule.agents.is_empty() {
        let agent_listed = rule.agents.iter().any(|candidate| {
            candidate == "*"
                || resource
                    .identity
                    .is_some_and(|identity| identity == candidate)
                || resource
                    .agent_id
                    .is_some_and(|agent_id| agent_id == candidate)
        });
        if !agent_listed {
            return false;
        }
    }
    if !rule.roles.is_empty() {
        let role_listed = resource.role.is_some_and(|role| {
            rule.roles
                .iter()
                .any(|candidate| candidate == "*" || candidate == role)
        });
        if !role_listed {
            return false;
        }
    }
    if !rule.match_labels.is_empty() {
        let Some(labels) = resource.labels else {
            // Unknown attributes fail label selectors closed.
            return false;
        };
        let all_labels_match = rule
            .match_labels
            .iter()
            .all(|(key, value)| labels.get(key) == Some(value));
        if !all_labels_match {
            return false;
        }
    }
    true
}

/// Evaluate one access check against a configuration.
///
/// Deny-by-default with deny-overrides: a matching deny rule always wins,
/// then a matching allow rule allows, otherwise the check denies. Admin
/// subjects bypass rules entirely; a disabled config allows everything.
pub fn evaluate_access(
    config: &AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
    resource: &AccessResource<'_>,
) -> AccessDecision {
    evaluate_access_lineage(config, principal, action, std::slice::from_ref(resource))
}

/// Evaluate one access check against an agent and its spawn lineage.
///
/// `resources` is the agent followed by its spawn ancestors (parent,
/// grandparent, ...). A rule applies when it matches *any* resource in the
/// chain, so permissions granted on a spawning agent extend to the members
/// it spawned — and a deny anywhere in the lineage denies the descendant
/// (deny-overrides is preserved across the chain).
pub(crate) fn evaluate_access_lineage(
    config: &AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
    resources: &[AccessResource<'_>],
) -> AccessDecision {
    if !config.enabled {
        return AccessDecision::Allow;
    }
    if let Some(subject) = principal.subject.as_deref()
        && config.admins.iter().any(|admin| admin == subject)
    {
        return AccessDecision::Allow;
    }

    let mut allowed = false;
    for rule in &config.rules {
        let matches = rule
            .actions
            .iter()
            .any(|pattern| action_matches(pattern, action))
            && subject_matches(rule, principal)
            && resources
                .iter()
                .any(|resource| resource_matches(rule, resource));
        if !matches {
            continue;
        }
        match rule.effect {
            AccessEffect::Deny => {
                return AccessDecision::Deny {
                    reason: format!("denied by rule {}", rule.id),
                };
            }
            AccessEffect::Allow => allowed = true,
        }
    }
    if allowed {
        AccessDecision::Allow
    } else {
        AccessDecision::Deny {
            reason: format!("no rule allows {action}"),
        }
    }
}

/// The first deny rule that matches `resource` for this principal and
/// action, ignoring allow rules. An admin matches none, as in
/// [`evaluate_access_lineage`].
pub(crate) fn matching_deny_rule<'c>(
    config: &'c AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
    resource: &AccessResource<'_>,
) -> Option<&'c str> {
    if !config.enabled {
        return None;
    }
    if let Some(subject) = principal.subject.as_deref()
        && config.admins.iter().any(|admin| admin == subject)
    {
        return None;
    }
    config
        .rules
        .iter()
        .find(|rule| {
            matches!(rule.effect, AccessEffect::Deny)
                && rule
                    .actions
                    .iter()
                    .any(|pattern| action_matches(pattern, action))
                && subject_matches(rule, principal)
                && resource_matches(rule, resource)
        })
        .map(|rule| rule.id.as_str())
}

/// True when the principal could perform `action` against at least one
/// resource — i.e. some allow rule matches the principal (by subject/group)
/// and names the action. Ignores resource selectors and deny rules, so it is
/// a coarse "is this affordance available at all" signal for capability
/// advertisement; per-resource checks (including deny-overrides) still apply
/// at call time. Always true when enforcement is disabled.
pub(crate) fn principal_may_perform(
    config: &AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
) -> bool {
    if !config.enabled {
        return true;
    }
    config.rules.iter().any(|rule| {
        matches!(rule.effect, AccessEffect::Allow)
            && rule
                .actions
                .iter()
                .any(|pattern| action_matches(pattern, action))
            && subject_matches(rule, principal)
    })
}

/// Resolve the configured group memberships for a subject.
pub(crate) fn groups_for_subject(config: &AccessControlConfig, subject: &str) -> BTreeSet<String> {
    config
        .groups
        .iter()
        .filter(|(_, group)| group.members.iter().any(|member| member == subject))
        .map(|(name, _)| name.clone())
        .collect()
}

/// The first deny rule that applies to this principal and action and that
/// the bare `identity` cannot decide: a rule whose resource selectors could
/// match through attributes a caller does not hold for that agent (its role,
/// its labels, or a spawn ancestor it names). An admin matches none, as in
/// [`evaluate_access_lineage`].
///
/// A record whose member attributes were never captured is refused under
/// such a rule (fails closed) instead of being judged as a bare identity,
/// which no role or label selector can match.
pub(crate) fn deny_undecidable_by_identity<'c>(
    config: &'c AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
    identity: &str,
) -> Option<&'c str> {
    if !config.enabled {
        return None;
    }
    if let Some(subject) = principal.subject.as_deref()
        && config.admins.iter().any(|admin| admin == subject)
    {
        return None;
    }
    config
        .rules
        .iter()
        .find(|rule| {
            matches!(rule.effect, AccessEffect::Deny)
                && rule
                    .actions
                    .iter()
                    .any(|pattern| action_matches(pattern, action))
                && subject_matches(rule, principal)
                && selectors_need_attributes(rule, identity)
        })
        .map(|rule| rule.id.as_str())
}

/// Whether a rule's resource selectors can match an agent through anything
/// but its bare identity. Every field of [`AccessRule`] is named here, so a
/// new selector kind does not compile until it is classified: a selector the
/// identity string alone cannot decide must answer `true`.
fn selectors_need_attributes(rule: &AccessRule, identity: &str) -> bool {
    let AccessRule {
        id: _,
        description: _,
        effect: _,
        subjects: _,
        groups: _,
        actions: _,
        agents,
        roles,
        match_labels,
    } = rule;
    // Roles and labels are attributes, never part of an identity string.
    let role_selector = !roles.is_empty();
    let label_selector = !match_labels.is_empty();
    // `*` and this identity are decided by the identity itself; any other
    // name may be a spawn ancestor (lineage) or a runtime id of the member.
    let lineage_selector = agents.iter().any(|agent| agent != "*" && agent != identity);
    role_selector || label_selector || lineage_selector
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::access::model::AccessGroup;

    fn config_with_rules(rules: Vec<AccessRule>) -> AccessControlConfig {
        AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            groups: BTreeMap::from([(
                "ops".to_string(),
                AccessGroup {
                    description: None,
                    members: vec!["alice@example.test".to_string()],
                },
            )]),
            rules,
            approvers: BTreeMap::new(),
        }
    }

    fn principal(subject: &str, groups: &[&str]) -> AccessPrincipal {
        AccessPrincipal {
            subject: Some(subject.to_string()),
            groups: groups.iter().map(ToString::to_string).collect(),
        }
    }

    fn allow_rule(id: &str) -> AccessRule {
        AccessRule {
            id: id.to_string(),
            ..AccessRule::default()
        }
    }

    #[test]
    fn disabled_config_allows_everything() {
        let config = AccessControlConfig::default();
        let decision = evaluate_access(
            &config,
            &AccessPrincipal::anonymous(),
            "access.admin",
            &AccessResource::none(),
        );
        assert!(decision.is_allow());
    }

    #[test]
    fn enabled_config_denies_by_default() {
        let config = config_with_rules(vec![]);
        let decision = evaluate_access(
            &config,
            &principal("bob@example.test", &[]),
            "agent.view",
            &AccessResource::for_identity("identity:ops-lead"),
        );
        assert!(!decision.is_allow());
    }

    #[test]
    fn admins_bypass_rules() {
        let config = config_with_rules(vec![]);
        let decision = evaluate_access(
            &config,
            &principal("root@example.test", &[]),
            "access.admin",
            &AccessResource::none(),
        );
        assert!(decision.is_allow());
    }

    #[test]
    fn group_can_view_all_but_send_to_one() {
        let mut view_rule = allow_rule("ops-view-all");
        view_rule.groups = vec!["ops".to_string()];
        view_rule.actions = vec!["agent.view".to_string()];
        let mut send_rule = allow_rule("ops-send-lead");
        send_rule.groups = vec!["ops".to_string()];
        send_rule.actions = vec!["agent.send".to_string()];
        send_rule.agents = vec!["identity:ops-lead".to_string()];
        let config = config_with_rules(vec![view_rule, send_rule]);
        let alice = principal("alice@example.test", &["ops"]);

        let can_view_any = evaluate_access(
            &config,
            &alice,
            "agent.view",
            &AccessResource::for_identity("identity:scout-1"),
        );
        assert!(can_view_any.is_allow());
        let can_send_lead = evaluate_access(
            &config,
            &alice,
            "agent.send",
            &AccessResource::for_identity("identity:ops-lead"),
        );
        assert!(can_send_lead.is_allow());
        let cannot_send_other = evaluate_access(
            &config,
            &alice,
            "agent.send",
            &AccessResource::for_identity("identity:scout-1"),
        );
        assert!(!cannot_send_other.is_allow());
    }

    #[test]
    fn deny_overrides_allow() {
        let mut allow_all = allow_rule("everyone-views");
        allow_all.actions = vec!["agent.view".to_string()];
        let mut deny_secret = allow_rule("hide-secret");
        deny_secret.effect = AccessEffect::Deny;
        deny_secret.actions = vec!["agent.*".to_string()];
        deny_secret.agents = vec!["identity:secret".to_string()];
        let config = config_with_rules(vec![allow_all, deny_secret]);
        let bob = principal("bob@example.test", &[]);

        assert!(
            evaluate_access(
                &config,
                &bob,
                "agent.view",
                &AccessResource::for_identity("identity:scout-1"),
            )
            .is_allow()
        );
        assert!(
            !evaluate_access(
                &config,
                &bob,
                "agent.view",
                &AccessResource::for_identity("identity:secret"),
            )
            .is_allow()
        );
    }

    #[test]
    fn agent_and_role_selectors_support_wildcard() {
        let mut agents_wildcard = allow_rule("view-any-agent");
        agents_wildcard.actions = vec!["agent.view".to_string()];
        agents_wildcard.agents = vec!["*".to_string()];
        let mut roles_wildcard = allow_rule("send-any-role");
        roles_wildcard.actions = vec!["agent.send".to_string()];
        roles_wildcard.roles = vec!["*".to_string()];
        let config = config_with_rules(vec![agents_wildcard, roles_wildcard]);
        let bob = principal("bob@example.test", &[]);

        assert!(
            evaluate_access(
                &config,
                &bob,
                "agent.view",
                &AccessResource::for_identity("identity:anyone"),
            )
            .is_allow()
        );
        // roles: ["*"] matches any known role...
        let with_role = AccessResource {
            identity: Some("identity:anyone"),
            role: Some("scout"),
            ..AccessResource::default()
        };
        assert!(evaluate_access(&config, &bob, "agent.send", &with_role).is_allow());
        // ...but stays closed when the role attribute is unknown.
        assert!(
            !evaluate_access(
                &config,
                &bob,
                "agent.send",
                &AccessResource::for_identity("identity:anyone"),
            )
            .is_allow()
        );
    }

    #[test]
    fn label_selectors_require_known_labels() {
        let mut rule = allow_rule("payments-only");
        rule.actions = vec!["agent.view".to_string()];
        rule.match_labels = BTreeMap::from([("org".to_string(), "payments".to_string())]);
        let config = config_with_rules(vec![rule]);
        let bob = principal("bob@example.test", &[]);

        let labels = BTreeMap::from([("org".to_string(), "payments".to_string())]);
        let with_labels = AccessResource {
            identity: Some("identity:pay-1"),
            labels: Some(&labels),
            ..AccessResource::default()
        };
        assert!(evaluate_access(&config, &bob, "agent.view", &with_labels).is_allow());

        let unknown_labels = AccessResource::for_identity("identity:pay-1");
        assert!(!evaluate_access(&config, &bob, "agent.view", &unknown_labels).is_allow());
    }

    #[test]
    fn resourceless_checks_only_match_unconstrained_rules() {
        let mut constrained = allow_rule("constrained-decide");
        constrained.actions = vec!["gating.decide".to_string()];
        constrained.agents = vec!["identity:ops-lead".to_string()];
        let config = config_with_rules(vec![constrained]);
        let bob = principal("bob@example.test", &[]);
        assert!(
            !evaluate_access(&config, &bob, "gating.decide", &AccessResource::none()).is_allow()
        );

        let mut unconstrained = allow_rule("decide");
        unconstrained.actions = vec!["gating.decide".to_string()];
        let config = config_with_rules(vec![unconstrained]);
        assert!(
            evaluate_access(&config, &bob, "gating.decide", &AccessResource::none()).is_allow()
        );
    }

    #[test]
    fn anonymous_matches_only_unconstrained_subjects() {
        let mut open_rule = allow_rule("everyone-views");
        open_rule.actions = vec!["agent.view".to_string()];
        let mut named_rule = allow_rule("alice-sends");
        named_rule.subjects = vec!["alice@example.test".to_string()];
        named_rule.actions = vec!["agent.send".to_string()];
        let config = config_with_rules(vec![open_rule, named_rule]);
        let anonymous = AccessPrincipal::anonymous();

        assert!(
            evaluate_access(
                &config,
                &anonymous,
                "agent.view",
                &AccessResource::for_identity("identity:scout-1"),
            )
            .is_allow()
        );
        assert!(
            !evaluate_access(
                &config,
                &anonymous,
                "agent.send",
                &AccessResource::for_identity("identity:scout-1"),
            )
            .is_allow()
        );
    }

    #[test]
    fn wildcard_subject_matches_authenticated_but_not_anonymous() {
        let mut star = allow_rule("any-authenticated");
        star.subjects = vec!["*".to_string()];
        star.actions = vec!["agent.view".to_string()];
        let config = config_with_rules(vec![star]);

        // Any authenticated subject matches subjects:["*"].
        assert!(
            evaluate_access(
                &config,
                &principal("carol@example.test", &[]),
                "agent.view",
                &AccessResource::for_identity("identity:scout-1"),
            )
            .is_allow()
        );
        // The anonymous principal does NOT — it only matches unconstrained rules.
        assert!(
            !evaluate_access(
                &config,
                &AccessPrincipal::anonymous(),
                "agent.view",
                &AccessResource::for_identity("identity:scout-1"),
            )
            .is_allow()
        );
    }

    #[test]
    fn groups_resolve_from_config() {
        let config = config_with_rules(vec![]);
        let groups = groups_for_subject(&config, "alice@example.test");
        assert!(groups.contains("ops"));
        assert!(groups_for_subject(&config, "bob@example.test").is_empty());
    }

    fn view_deny(id: &str) -> AccessRule {
        AccessRule {
            id: id.to_string(),
            effect: AccessEffect::Deny,
            actions: vec!["agent.view".to_string()],
            ..AccessRule::default()
        }
    }

    fn undecidable(rule: AccessRule, identity: &str) -> Option<String> {
        let config = config_with_rules(vec![allow_view_all(), rule]);
        deny_undecidable_by_identity(
            &config,
            &principal("bob@example.test", &[]),
            "agent.view",
            identity,
        )
        .map(ToString::to_string)
    }

    fn allow_view_all() -> AccessRule {
        AccessRule {
            id: "allow-view".to_string(),
            actions: vec!["agent.view".to_string()],
            ..AccessRule::default()
        }
    }

    // One case per selector kind of `agent.view`: role, label and lineage
    // (an agent name other than the record's own) cannot be decided from a
    // bare identity, so an uncaptured record fails closed under them.
    #[test]
    fn a_role_selector_deny_is_undecidable_by_identity() {
        let rule = AccessRule {
            roles: vec!["scout".to_string()],
            ..view_deny("deny-role")
        };
        assert_eq!(undecidable(rule, "worker").as_deref(), Some("deny-role"));
    }

    #[test]
    fn a_label_selector_deny_is_undecidable_by_identity() {
        let rule = AccessRule {
            match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
            ..view_deny("deny-label")
        };
        assert_eq!(undecidable(rule, "worker").as_deref(), Some("deny-label"));
    }

    #[test]
    fn a_deny_naming_another_agent_is_undecidable_by_identity() {
        let rule = AccessRule {
            agents: vec!["secret-lead".to_string()],
            ..view_deny("deny-ancestor")
        };
        assert_eq!(
            undecidable(rule, "worker").as_deref(),
            Some("deny-ancestor")
        );
    }

    #[test]
    fn denies_the_identity_decides_are_not_undecidable() {
        let own = AccessRule {
            agents: vec!["worker".to_string()],
            ..view_deny("deny-own")
        };
        assert_eq!(undecidable(own, "worker"), None);
        let every = AccessRule {
            agents: vec!["*".to_string()],
            ..view_deny("deny-every")
        };
        assert_eq!(undecidable(every, "worker"), None);
        // A selector-free deny matches every record, captured or not.
        assert_eq!(undecidable(view_deny("deny-all"), "worker"), None);
    }

    #[test]
    fn undecidable_denies_respect_action_subject_and_admin() {
        let label = AccessRule {
            match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
            ..view_deny("deny-label")
        };
        let config = config_with_rules(vec![label.clone()]);
        // Another action is not `agent.view`.
        assert_eq!(
            deny_undecidable_by_identity(
                &config,
                &principal("bob@example.test", &[]),
                "agent.send",
                "worker",
            ),
            None
        );
        // Admins bypass every rule.
        assert_eq!(
            deny_undecidable_by_identity(
                &config,
                &principal("root@example.test", &[]),
                "agent.view",
                "worker",
            ),
            None
        );
        // A rule scoped to another subject does not apply.
        let scoped = AccessRule {
            subjects: vec!["carol@example.test".to_string()],
            ..label
        };
        let config = config_with_rules(vec![scoped]);
        assert_eq!(
            deny_undecidable_by_identity(
                &config,
                &principal("bob@example.test", &[]),
                "agent.view",
                "worker",
            ),
            None
        );
        // An allow rule with the same selectors is never a deny.
        let allow = AccessRule {
            id: "allow-label".to_string(),
            actions: vec!["agent.view".to_string()],
            match_labels: BTreeMap::from([("tier".to_string(), "secret".to_string())]),
            ..AccessRule::default()
        };
        let config = config_with_rules(vec![allow]);
        assert_eq!(
            deny_undecidable_by_identity(
                &config,
                &principal("bob@example.test", &[]),
                "agent.view",
                "worker",
            ),
            None
        );
    }
}
