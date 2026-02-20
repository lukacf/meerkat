//! Pure topology policy evaluator.

use crate::definition::TopologyRule;
use crate::ids::ProfileName;

/// Rule evaluation output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

/// Evaluate topology allow/deny decision for a role edge.
///
/// Later matching rules win. If no rule matches, edge is allowed.
pub fn evaluate_topology(
    rules: &[TopologyRule],
    from_role: &ProfileName,
    to_role: &ProfileName,
) -> PolicyDecision {
    rules
        .iter()
        .filter(|rule| {
            role_matches(&rule.from_role, from_role) && role_matches(&rule.to_role, to_role)
        })
        .next_back()
        .map(|rule| {
            if rule.allowed {
                PolicyDecision::Allow
            } else {
                PolicyDecision::Deny
            }
        })
        .unwrap_or(PolicyDecision::Allow)
}

fn role_matches(rule_role: &ProfileName, actual_role: &ProfileName) -> bool {
    rule_role.as_str() == "*" || rule_role == actual_role
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology_defaults_to_allow() {
        let rules = vec![TopologyRule {
            from_role: ProfileName::from("lead"),
            to_role: ProfileName::from("worker"),
            allowed: true,
        }];
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("worker"),
                &ProfileName::from("reviewer")
            ),
            PolicyDecision::Allow
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
            PolicyDecision::Deny
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
            PolicyDecision::Allow
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
            PolicyDecision::Deny
        );
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("reviewer"),
                &ProfileName::from("worker")
            ),
            PolicyDecision::Deny
        );
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("reviewer"),
                &ProfileName::from("lead")
            ),
            PolicyDecision::Allow
        );
    }
}
