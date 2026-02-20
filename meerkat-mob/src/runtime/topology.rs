//! Pure topology policy evaluator.

use crate::definition::TopologyRule;

/// Rule evaluation output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

/// Evaluate topology allow/deny decision for a role edge.
///
/// Later matching rules win. If no rule matches, edge is allowed.
pub fn evaluate_topology(rules: &[TopologyRule], from_role: &str, to_role: &str) -> PolicyDecision {
    rules
        .iter()
        .filter(|rule| rule.from_role == from_role && rule.to_role == to_role)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topology_defaults_to_allow() {
        let rules = vec![TopologyRule {
            from_role: "lead".to_string(),
            to_role: "worker".to_string(),
            allowed: true,
        }];
        assert_eq!(
            evaluate_topology(&rules, "worker", "reviewer"),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn test_topology_can_deny_edge() {
        let rules = vec![TopologyRule {
            from_role: "lead".to_string(),
            to_role: "worker".to_string(),
            allowed: false,
        }];
        assert_eq!(
            evaluate_topology(&rules, "lead", "worker"),
            PolicyDecision::Deny
        );
    }

    #[test]
    fn test_topology_last_rule_wins() {
        let rules = vec![
            TopologyRule {
                from_role: "lead".to_string(),
                to_role: "worker".to_string(),
                allowed: false,
            },
            TopologyRule {
                from_role: "lead".to_string(),
                to_role: "worker".to_string(),
                allowed: true,
            },
        ];
        assert_eq!(
            evaluate_topology(&rules, "lead", "worker"),
            PolicyDecision::Allow
        );
    }
}
