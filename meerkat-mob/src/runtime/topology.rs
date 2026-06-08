//! Pure topology policy evaluator.
//!
//! `MobTopologyService` owns only the topology-policy spec. The
//! monotonic `topology_epoch` (and the coordinator-bound bit) live on
//! `MobMachine` as DSL state; the shell projects them from there. See
//! `meerkat_machine_schema::catalog::dsl::mob_machine` and
//! `meerkat-mob::machines::mob_machine` for the authoritative writes.

use crate::definition::{TopologyRule, TopologySpec};
use crate::ids::ProfileName;

const WILDCARD_ROLE: &str = "*";

/// Rule evaluation output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

/// Pure policy evaluator for topology rules. Holds the declarative
/// allow/deny spec; does not track any runtime state.
#[derive(Clone)]
pub struct MobTopologyService {
    spec: Option<TopologySpec>,
}

impl MobTopologyService {
    pub fn new(spec: Option<TopologySpec>) -> Self {
        Self { spec }
    }

    /// Pure rule-match projection for a role edge.
    ///
    /// Returns `Some(PolicyDecision)` only when the spec is present AND a rule
    /// matches the edge. `None` (no spec, or no matching rule) means "no rule
    /// projection" — the default-policy fallback is MobMachine-owned, not a
    /// shell allow-by-default.
    pub fn evaluate(
        &self,
        from_role: &ProfileName,
        to_role: &ProfileName,
    ) -> Option<PolicyDecision> {
        self.spec
            .as_ref()
            .and_then(|spec| evaluate_topology(&spec.rules, from_role, to_role))
    }
}

/// Evaluate the pure topology allow/deny rule-match for a role edge.
///
/// Later matching rules win. Returns `None` when no rule matches; the
/// machine — not the shell — applies the default policy for unmatched edges.
pub fn evaluate_topology(
    rules: &[TopologyRule],
    from_role: &ProfileName,
    to_role: &ProfileName,
) -> Option<PolicyDecision> {
    rules
        .iter()
        .rfind(|rule| {
            role_matches(&rule.from_role, from_role) && role_matches(&rule.to_role, to_role)
        })
        .map(|rule| {
            if rule.allowed {
                PolicyDecision::Allow
            } else {
                PolicyDecision::Deny
            }
        })
}

fn role_matches(rule_role: &ProfileName, actual_role: &ProfileName) -> bool {
    rule_role.as_str() == WILDCARD_ROLE || rule_role == actual_role
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology_unmatched_edge_has_no_rule_projection() {
        let rules = vec![TopologyRule {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("worker"),
            allowed: true,
        }];
        // No rule matches this edge — the shell projects `None` and leaves the
        // default-policy fallback to MobMachine.
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("worker"),
                &ProfileName::from("reviewer")
            ),
            None
        );
    }

    #[test]
    fn test_topology_can_deny_edge() {
        let rules = vec![TopologyRule {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("worker"),
            allowed: false,
        }];
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("lead"),
                &ProfileName::from("worker")
            ),
            Some(PolicyDecision::Deny)
        );
    }

    #[test]
    fn test_topology_last_rule_wins() {
        let rules = vec![
            TopologyRule {
                from_role: ProfileName::from("lead"),
                to_role: ProfileName::from("worker"),
                allowed: false,
            },
            TopologyRule {
                from_role: ProfileName::from("lead"),
                to_role: ProfileName::from("worker"),
                allowed: true,
            },
        ];
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("lead"),
                &ProfileName::from("worker")
            ),
            Some(PolicyDecision::Allow)
        );
    }

    #[test]
    fn test_topology_supports_wildcard_matching() {
        let rules = vec![TopologyRule {
            from_role: ProfileName::from("*"),
            to_role: ProfileName::from("worker"),
            allowed: false,
        }];
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("lead"),
                &ProfileName::from("worker")
            ),
            Some(PolicyDecision::Deny)
        );
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("reviewer"),
                &ProfileName::from("worker")
            ),
            Some(PolicyDecision::Deny)
        );
        // No rule matches this edge — no shell projection.
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("reviewer"),
                &ProfileName::from("lead")
            ),
            None
        );
    }

    #[test]
    fn test_topology_supports_to_role_wildcard_matching() {
        let rules = vec![TopologyRule {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("*"),
            allowed: false,
        }];
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("lead"),
                &ProfileName::from("worker")
            ),
            Some(PolicyDecision::Deny)
        );
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("lead"),
                &ProfileName::from("reviewer")
            ),
            Some(PolicyDecision::Deny)
        );
        // No rule matches this edge — no shell projection.
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("worker"),
                &ProfileName::from("reviewer")
            ),
            None
        );
    }
}
