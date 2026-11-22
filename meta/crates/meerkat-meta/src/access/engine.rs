//! Pure ABAC evaluation. No locks, no IO — config in, decision out.

use std::collections::{BTreeMap, BTreeSet};

use super::model::{AccessControlConfig, AccessEffect, AccessRule, action_pattern_matches};

/// The authenticated caller's attributes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccessPrincipal {
    /// Authenticated subject (email or token `sub`). `None` means the
    /// request was admitted without app auth (open console). Such a caller
    /// may be any person: it matches every rule without subject or group
    /// constraints and every deny rule that has them, and no allow rule
    /// that has them.
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
    /// Agent identity (preferred key, e.g. `ops-lead`).
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
    // An anonymous caller of an open console may be any person, including
    // one a rule names. A deny rule with subject or group constraints binds
    // it, so leaving out a token never escapes a deny. An allow rule with
    // them never grants it, `subjects: ["*"]` included: `"*"` matches any
    // *authenticated* subject, as `"*"` in the resource selectors never
    // matches an absent attribute.
    let Some(subject) = principal.subject.as_deref() else {
        return matches!(rule.effect, AccessEffect::Deny);
    };
    if rule
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

/// Whether one field of what is held of an agent is known. A field that is
/// not known is never read as "has none": a rule keyed on it is decided
/// closed ([`resource_match`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Knowledge {
    /// The field is as held: a role that is absent is no role, a label key
    /// that is absent is a label the agent does not carry, a spawner that
    /// is absent is none.
    #[default]
    Known,
    /// The field is not known. What is held of it is still true as far as
    /// it goes: a label is held only where the console wrote it itself
    /// (who the agent is private to), so a held label is one the agent
    /// carries. A value a spawn call carried is never held so.
    NotKnown,
}

/// Which fields of an agent's attributes are known, each on its own: its
/// role, its labels, and who spawned it (its lineage above itself).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AttributeKnowledge {
    pub(crate) role: Knowledge,
    pub(crate) labels: Knowledge,
    pub(crate) spawner: Knowledge,
}

impl AttributeKnowledge {
    /// Every field is known: the attributes are of the whole agent.
    pub(crate) const WHOLE: Self = Self {
        role: Knowledge::Known,
        labels: Knowledge::Known,
        spawner: Knowledge::Known,
    };

    /// Nothing is known beyond the identity and the labels that are held:
    /// not the role, not any other label, not who spawned the agent.
    pub(crate) const IDENTITY_AND_HELD_LABELS: Self = Self {
        role: Knowledge::NotKnown,
        labels: Knowledge::NotKnown,
        spawner: Knowledge::NotKnown,
    };
}

/// A resource with what is known of each of its fields.
#[derive(Debug, Clone, Copy)]
pub(crate) struct KnownResource<'a> {
    pub(crate) resource: AccessResource<'a>,
    pub(crate) knowledge: AttributeKnowledge,
}

impl<'a> KnownResource<'a> {
    /// A resource whose every field is known as given.
    pub(crate) fn whole(resource: AccessResource<'a>) -> Self {
        Self {
            resource,
            knowledge: AttributeKnowledge::WHOLE,
        }
    }
}

/// What a rule's resource selectors say of a resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectorMatch {
    /// Every selector is decided by what is known, and the rule matches.
    Matches,
    /// A selector is decided by what is known, and the rule does not match.
    DoesNotMatch,
    /// No selector rules the resource out, and one of them reads a field
    /// that is not known: its role, a label it may carry, or another agent
    /// that may be its spawn ancestor.
    NotKnown,
}

/// How `rule`'s resource selectors read against `resource`, field by field:
/// the one place that matches a rule against a resource. A selector on a
/// field that is known decides as the field says. A selector on a field
/// that is not known decides nothing: a role that is not known is never
/// "has no role", a label key that is not held is never "has none" (a held
/// label with another value still rules the rule out), and, where the
/// spawner is not known, an agent the rule names may be a spawn ancestor.
/// Every field of [`AccessRule`] is named, so a new selector kind does not
/// compile until it is classified.
fn resource_match(rule: &AccessRule, known: &KnownResource<'_>) -> SelectorMatch {
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
    if agents.is_empty() && roles.is_empty() && match_labels.is_empty() {
        return SelectorMatch::Matches;
    }
    let KnownResource {
        resource,
        knowledge,
    } = known;
    // A constrained rule can never match a check that has no resource.
    if !resource.is_present() {
        return SelectorMatch::DoesNotMatch;
    }
    let mut not_known = false;
    if !agents.is_empty() {
        let agent_listed = agents.iter().any(|candidate| {
            candidate == "*"
                || resource
                    .identity
                    .is_some_and(|identity| identity == candidate)
                || resource
                    .agent_id
                    .is_some_and(|agent_id| agent_id == candidate)
        });
        if !agent_listed {
            match knowledge.spawner {
                Knowledge::Known => return SelectorMatch::DoesNotMatch,
                // Any other name may be a spawn ancestor of the agent.
                Knowledge::NotKnown => not_known = true,
            }
        }
    }
    if !roles.is_empty() {
        match knowledge.role {
            Knowledge::Known => {
                let role_listed = resource.role.is_some_and(|role| {
                    roles
                        .iter()
                        .any(|candidate| candidate == "*" || candidate == role)
                });
                if !role_listed {
                    return SelectorMatch::DoesNotMatch;
                }
            }
            Knowledge::NotKnown => not_known = true,
        }
    }
    for (key, value) in match_labels {
        match resource.labels.and_then(|labels| labels.get(key)) {
            Some(held) if held == value => {}
            Some(_) => return SelectorMatch::DoesNotMatch,
            // No labels at all (attributes never captured) and a key that
            // is not held fail a label selector closed when the labels are
            // known, and decide nothing when they are not.
            None => match knowledge.labels {
                Knowledge::Known => return SelectorMatch::DoesNotMatch,
                Knowledge::NotKnown => not_known = true,
            },
        }
    }
    if not_known {
        SelectorMatch::NotKnown
    } else {
        SelectorMatch::Matches
    }
}

/// Whether a deny rule applies to `known`: its selectors match, or cannot
/// be decided by what is known (closed).
fn deny_applies(rule: &AccessRule, known: &KnownResource<'_>) -> bool {
    resource_match(rule, known) != SelectorMatch::DoesNotMatch
}

/// Whether an allow rule applies to `known`: every selector is decided by
/// what is known, and the rule matches (closed).
fn allow_applies(rule: &AccessRule, known: &KnownResource<'_>) -> bool {
    resource_match(rule, known) == SelectorMatch::Matches
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
    evaluate_access_lineage(
        config,
        principal,
        action,
        &[KnownResource::whole(*resource)],
        &|_| true,
    )
}

/// Evaluate one access check against an agent and its lineage.
///
/// `resources` is the agent followed by its lineage ancestors (parent,
/// grandparent, ...), each with what is known of its fields. A deny rule
/// applies when it matches *any* resource in the chain, so a deny anywhere
/// in the lineage denies the descendant (deny-overrides is preserved across
/// the chain). An allow rule applies when it matches a resource in the
/// chain that `grants` lets grant, so permissions granted on a spawning
/// agent extend to the members it spawned. A field that is not known is
/// decided closed in both directions ([`resource_match`]): a deny applies
/// to a resource it matches or cannot rule out, an allow only to one it
/// matches by what is known.
/// A caller passes `&|_| true` unless a link of its chain takes no grant of
/// its own (the console's side chats,
/// `AccessView::decide_agent`).
pub(crate) fn evaluate_access_lineage(
    config: &AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
    resources: &[KnownResource<'_>],
    grants: &dyn Fn(&AccessResource<'_>) -> bool,
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
        let applies = rule
            .actions
            .iter()
            .any(|pattern| action_matches(pattern, action))
            && subject_matches(rule, principal);
        if !applies {
            continue;
        }
        match rule.effect {
            AccessEffect::Deny => {
                if resources.iter().any(|known| deny_applies(rule, known)) {
                    return AccessDecision::Deny {
                        reason: format!("denied by rule {}", rule.id),
                    };
                }
            }
            AccessEffect::Allow => {
                if resources
                    .iter()
                    .any(|known| grants(&known.resource) && allow_applies(rule, known))
                {
                    allowed = true;
                }
            }
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

/// The first deny rule that applies to `resource` for this principal and
/// action, ignoring allow rules: one that matches it or, where a field of
/// it is not known, one that what is known cannot rule out. An admin
/// matches none, as in [`evaluate_access_lineage`].
pub(crate) fn matching_deny_rule<'c>(
    config: &'c AccessControlConfig,
    principal: &AccessPrincipal,
    action: &str,
    resource: &KnownResource<'_>,
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
                && deny_applies(rule, resource)
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

    /// An agent of which only its identity and some labels are known is
    /// decided closed by what is not known, in both directions. An allow
    /// applies only when every selector is decided: by `*`, by the
    /// identity, or by a label that is known. A deny applies also when a
    /// selector reads the role, a label that is not among the known ones,
    /// or another agent that may be a spawn ancestor; a deny on a known
    /// label with another value, or with a known label that differs, does
    /// not apply. An admin is allowed, and a disabled configuration allows.
    #[test]
    fn an_agent_known_in_part_is_decided_closed_by_what_is_not_known() {
        const AGENT: &str = "aide";
        let known = BTreeMap::from([("kept".to_string(), "team".to_string())]);
        let caller = principal("bob@example.test", &[]);
        // The agent as the console records one that ended and that only
        // the record of who each member is private to knows.
        let agent = [KnownResource {
            resource: AccessResource {
                identity: Some(AGENT),
                agent_id: Some(AGENT),
                role: None,
                labels: Some(&known),
            },
            knowledge: AttributeKnowledge::IDENTITY_AND_HELD_LABELS,
        }];
        let view = |rules: Vec<AccessRule>| {
            evaluate_access_lineage(
                &config_with_rules(rules),
                &caller,
                "agent.view",
                &agent,
                &|_| true,
            )
            .is_allow()
        };
        let allow = |id: &str| AccessRule {
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            ..allow_rule(id)
        };
        let deny = |id: &str| AccessRule {
            effect: AccessEffect::Deny,
            ..allow(id)
        };
        let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
        let everything = AccessRule {
            agents: vec!["*".to_string()],
            ..allow("everything")
        };

        // Allows: only what is decided grants.
        assert!(view(vec![everything.clone()]), "an allow by `*`");
        assert!(
            view(vec![AccessRule {
                agents: vec![AGENT.to_string()],
                ..allow("by-identity")
            }]),
            "an allow by the identity"
        );
        assert!(
            view(vec![AccessRule {
                match_labels: label("kept", "team"),
                ..allow("by-the-known-label")
            }]),
            "an allow by a label that is known"
        );
        for (rule, why) in [
            (
                AccessRule {
                    roles: vec!["worker".to_string()],
                    ..allow("by-role")
                },
                "an allow by role",
            ),
            (
                AccessRule {
                    roles: vec!["*".to_string()],
                    ..allow("by-any-role")
                },
                "an allow by any role",
            ),
            (
                AccessRule {
                    match_labels: label("org", "payments"),
                    ..allow("by-another-label")
                },
                "an allow by a label that is not known",
            ),
            (
                AccessRule {
                    match_labels: label("kept", "other"),
                    ..allow("by-the-known-label-with-another-value")
                },
                "an allow by the known label with another value",
            ),
            (
                AccessRule {
                    agents: vec!["builder".to_string()],
                    ..allow("by-another-agent")
                },
                "an allow by another agent's name",
            ),
        ] {
            assert!(!view(vec![rule]), "{why} grants nothing");
        }

        // Denies: what cannot be decided refuses.
        for (rule, why) in [
            (
                AccessRule {
                    roles: vec!["worker".to_string()],
                    ..deny("by-role")
                },
                "a deny by role",
            ),
            (
                AccessRule {
                    match_labels: label("org", "payments"),
                    ..deny("by-another-label")
                },
                "a deny by a label that is not known",
            ),
            (
                AccessRule {
                    agents: vec!["builder".to_string()],
                    ..deny("by-another-agent")
                },
                "a deny by another agent's name",
            ),
            (
                AccessRule {
                    match_labels: label("kept", "team"),
                    ..deny("by-the-known-label")
                },
                "a deny by the label that is known",
            ),
            (
                AccessRule {
                    match_labels: BTreeMap::from([
                        ("kept".to_string(), "team".to_string()),
                        ("org".to_string(), "payments".to_string()),
                    ]),
                    ..deny("by-the-known-label-and-another")
                },
                "a deny by the known label and one that is not known",
            ),
        ] {
            assert!(
                !view(vec![everything.clone(), rule]),
                "{why} refuses, with another allow present"
            );
        }
        for (rule, why) in [
            (
                AccessRule {
                    match_labels: label("kept", "other"),
                    ..deny("by-the-known-label-with-another-value")
                },
                "a deny by the known label with another value",
            ),
            (
                AccessRule {
                    match_labels: BTreeMap::from([
                        ("kept".to_string(), "other".to_string()),
                        ("org".to_string(), "payments".to_string()),
                    ]),
                    ..deny("by-a-known-label-that-differs-and-another")
                },
                "a deny whose known label differs",
            ),
        ] {
            assert!(view(vec![everything.clone(), rule]), "{why} does not apply");
        }

        // An admin, and a disabled configuration.
        let refused = vec![AccessRule {
            roles: vec!["worker".to_string()],
            ..deny("by-role")
        }];
        assert!(
            evaluate_access_lineage(
                &config_with_rules(refused.clone()),
                &principal("root@example.test", &[]),
                "agent.view",
                &agent,
                &|_| true,
            )
            .is_allow(),
            "an admin is allowed"
        );
        let mut disabled = config_with_rules(refused);
        disabled.enabled = false;
        assert!(
            evaluate_access_lineage(&disabled, &caller, "agent.view", &agent, &|_| true).is_allow(),
            "a disabled configuration allows"
        );
    }

    /// What is known of an agent is known field by field. An agent whose
    /// role alone is not known is decided closed by a rule on a role, in
    /// both directions, and by its labels and its spawner as they are held:
    /// a label that is not held is a label it does not carry, and an agent
    /// that is not among its ancestors is not its ancestor.
    #[test]
    fn a_role_that_is_not_known_is_decided_closed_and_the_other_fields_as_held() {
        let caller = principal("bob@example.test", &[]);
        let own_labels = BTreeMap::from([("tier".to_string(), "vault".to_string())]);
        let planner_labels = BTreeMap::new();
        // The agent, its role not known, and the agent that spawned it.
        let role_alone = AttributeKnowledge {
            role: Knowledge::NotKnown,
            ..AttributeKnowledge::WHOLE
        };
        let chain = [
            KnownResource {
                resource: AccessResource {
                    identity: Some("aide"),
                    agent_id: Some("aide"),
                    role: None,
                    labels: Some(&own_labels),
                },
                knowledge: role_alone,
            },
            KnownResource::whole(AccessResource {
                identity: Some("planner"),
                agent_id: Some("planner"),
                role: Some("lead"),
                labels: Some(&planner_labels),
            }),
        ];
        let view = |rules: Vec<AccessRule>| {
            evaluate_access_lineage(
                &config_with_rules(rules),
                &caller,
                "agent.view",
                &chain,
                &|_| true,
            )
            .is_allow()
        };
        let allow = |id: &str| AccessRule {
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            ..allow_rule(id)
        };
        let deny = |id: &str| AccessRule {
            effect: AccessEffect::Deny,
            ..allow(id)
        };
        let label = |key: &str, value: &str| BTreeMap::from([(key.to_string(), value.to_string())]);
        let everything = AccessRule {
            agents: vec!["*".to_string()],
            ..allow("everything")
        };

        // The role: closed in both directions.
        assert!(
            !view(vec![
                everything.clone(),
                AccessRule {
                    roles: vec!["worker".to_string()],
                    ..deny("by-role")
                }
            ]),
            "a deny by role refuses, with another allow present"
        );
        assert!(
            !view(vec![AccessRule {
                roles: vec!["worker".to_string()],
                ..allow("by-role")
            }]),
            "an allow by role grants nothing"
        );
        // The labels as held.
        assert!(
            view(vec![AccessRule {
                match_labels: label("tier", "vault"),
                ..allow("by-a-held-label")
            }]),
            "an allow by a held label"
        );
        assert!(
            !view(vec![AccessRule {
                match_labels: label("org", "payments"),
                ..allow("by-a-label-not-held")
            }]),
            "an allow by a label that is not held grants nothing"
        );
        assert!(
            !view(vec![
                everything.clone(),
                AccessRule {
                    match_labels: label("tier", "vault"),
                    ..deny("by-a-held-label")
                }
            ]),
            "a deny by a held label refuses"
        );
        assert!(
            view(vec![
                everything.clone(),
                AccessRule {
                    match_labels: label("org", "payments"),
                    ..deny("by-a-label-not-held")
                }
            ]),
            "a deny by a label that is not held does not apply: the labels are known"
        );
        // The spawner as held.
        assert!(
            view(vec![AccessRule {
                agents: vec!["planner".to_string()],
                ..allow("by-the-spawner")
            }]),
            "an allow on the spawner"
        );
        assert!(
            !view(vec![
                everything.clone(),
                AccessRule {
                    agents: vec!["planner".to_string()],
                    ..deny("by-the-spawner")
                }
            ]),
            "a deny on the spawner refuses"
        );
        assert!(
            view(vec![
                everything,
                AccessRule {
                    agents: vec!["builder".to_string()],
                    ..deny("by-another-agent")
                }
            ]),
            "a deny on an agent that is not an ancestor does not apply: the spawner is known"
        );
    }

    /// A field that is not known is decided closed on every link of a
    /// lineage, an ancestor's as the agent's own: a deny on a role refuses
    /// an agent one of whose ancestors has a role that is not known, and an
    /// allow on a role takes nothing from that ancestor. An ancestor of
    /// which nothing but its name is known refuses the agent under a deny
    /// on a label and under a deny that names another agent.
    #[test]
    fn an_ancestor_with_a_field_that_is_not_known_is_decided_closed() {
        let caller = principal("bob@example.test", &[]);
        let none = BTreeMap::new();
        let child = KnownResource::whole(AccessResource {
            identity: Some("apprentice"),
            agent_id: Some("apprentice"),
            role: Some("reader"),
            labels: Some(&none),
        });
        let ancestor = |knowledge| KnownResource {
            resource: AccessResource {
                identity: Some("aide"),
                agent_id: None,
                role: None,
                labels: Some(&none),
            },
            knowledge,
        };
        let view = |knowledge, rules: Vec<AccessRule>| {
            evaluate_access_lineage(
                &config_with_rules(rules),
                &caller,
                "agent.view",
                &[child, ancestor(knowledge)],
                &|_| true,
            )
            .is_allow()
        };
        let allow = |id: &str| AccessRule {
            subjects: vec!["*".to_string()],
            actions: vec!["agent.*".to_string()],
            ..allow_rule(id)
        };
        let deny = |id: &str| AccessRule {
            effect: AccessEffect::Deny,
            ..allow(id)
        };
        let everything = AccessRule {
            agents: vec!["*".to_string()],
            ..allow("everything")
        };
        let by_role = AccessRule {
            roles: vec!["worker".to_string()],
            ..deny("by-role")
        };
        let by_label = AccessRule {
            match_labels: BTreeMap::from([("tier".to_string(), "vault".to_string())]),
            ..deny("by-label")
        };
        let by_another_agent = AccessRule {
            agents: vec!["builder".to_string()],
            ..deny("by-another-agent")
        };

        let whole = AttributeKnowledge::WHOLE;
        let role_not_known = AttributeKnowledge {
            role: Knowledge::NotKnown,
            ..AttributeKnowledge::WHOLE
        };
        let name_only = AttributeKnowledge::IDENTITY_AND_HELD_LABELS;
        assert!(
            view(whole, vec![everything.clone(), by_role.clone()]),
            "an ancestor known to have no role is not a worker"
        );
        for knowledge in [role_not_known, name_only] {
            assert!(
                !view(knowledge, vec![everything.clone(), by_role.clone()]),
                "a deny by role refuses the child of an ancestor whose role is not known"
            );
            assert!(
                !view(
                    knowledge,
                    vec![AccessRule {
                        roles: vec!["worker".to_string()],
                        ..allow("by-role")
                    }]
                ),
                "an allow by role takes nothing from an ancestor whose role is not known"
            );
        }
        assert!(
            view(role_not_known, vec![everything.clone(), by_label.clone()])
                && view(
                    role_not_known,
                    vec![everything.clone(), by_another_agent.clone()]
                ),
            "an ancestor whose labels and spawner are held is decided by them"
        );
        assert!(
            !view(name_only, vec![everything.clone(), by_label])
                && !view(name_only, vec![everything, by_another_agent]),
            "an ancestor of which only the name is known refuses under a deny on a label \
             and under a deny that names another agent"
        );
    }

    /// The engine knows no side chats: a single resource that carries a
    /// `side_chat_of` label is decided by the rules that match it, as any
    /// resource is (extension state and announcement audiences decide that
    /// way). Only the console's lineage decisions take no grant from a side
    /// chat's own link (`AccessView::decide_agent`).
    #[test]
    fn a_side_chat_label_changes_nothing_in_a_single_resource_decision() {
        let config = config_with_rules(vec![AccessRule {
            agents: vec!["alpha-side".to_string()],
            actions: vec!["extension.read".to_string()],
            ..allow_rule("read-the-side-chat")
        }]);
        let labels = BTreeMap::from([("side_chat_of".to_string(), "alpha".to_string())]);
        let resource = AccessResource {
            identity: Some("alpha-side"),
            agent_id: None,
            role: None,
            labels: Some(&labels),
        };
        assert!(
            evaluate_access(
                &config,
                &principal("bob@example.test", &[]),
                "extension.read",
                &resource,
            )
            .is_allow()
        );
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

    /// An open Console admits callers without a token, and an anonymous
    /// caller may be any person: a deny rule that names a subject or a group
    /// binds it too, so leaving out a token never escapes a deny. An allow
    /// rule that names people still never matches it.
    #[test]
    fn an_anonymous_caller_is_bound_by_deny_rules_that_name_people() {
        let deny = |id: &str, subjects: &[&str], groups: &[&str], action: &str| AccessRule {
            id: id.to_string(),
            effect: AccessEffect::Deny,
            subjects: subjects.iter().map(ToString::to_string).collect(),
            groups: groups.iter().map(ToString::to_string).collect(),
            actions: vec![action.to_string()],
            ..AccessRule::default()
        };
        let allow = |id: &str, subjects: &[&str], action: &str| AccessRule {
            id: id.to_string(),
            subjects: subjects.iter().map(ToString::to_string).collect(),
            actions: vec![action.to_string()],
            ..AccessRule::default()
        };
        let config = config_with_rules(vec![
            allow("everyone-retires", &[], "agent.retire"),
            deny(
                "alice-never-retires",
                &["alice@example.test"],
                &[],
                "agent.retire",
            ),
            allow("everyone-resets", &[], "agent.reset"),
            deny("ops-never-resets", &[], &["ops"], "agent.reset"),
            deny("no-signed-in-spawn", &["*"], &[], "agent.respawn"),
            allow("everyone-respawns", &[], "agent.respawn"),
            allow("bob-spawns", &["bob@example.test"], "agent.spawn"),
        ]);
        let allows = |principal: &AccessPrincipal, action: &str| {
            evaluate_access(&config, principal, action, &AccessResource::none()).is_allow()
        };
        let anonymous = AccessPrincipal::anonymous();
        let carol = principal("carol@example.test", &[]);
        assert_eq!(
            [
                allows(&anonymous, "agent.retire"),
                allows(&anonymous, "agent.reset"),
                allows(&anonymous, "agent.respawn"),
                allows(&anonymous, "agent.spawn"),
            ],
            [false; 4],
            "every deny that names people binds an anonymous caller, and an allow that \
             names people never matches it"
        );
        assert_eq!(
            [
                allows(&carol, "agent.retire"),
                allows(&carol, "agent.reset"),
                allows(&principal("alice@example.test", &[]), "agent.retire"),
                allows(&principal("dave@example.test", &["ops"]), "agent.reset"),
            ],
            [true, true, false, false],
            "named callers are decided as before"
        );
    }
}
