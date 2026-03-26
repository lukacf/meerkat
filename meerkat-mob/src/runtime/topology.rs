//! Pure topology policy evaluator.

use crate::definition::{TopologyRule, TopologySpec};
use crate::ids::ProfileName;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const WILDCARD_ROLE: &str = "*";

/// Rule evaluation output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny,
}

/// Explicit owner for topology policy and monotonic revision tracking.
#[derive(Clone)]
pub struct MobTopologyService {
    spec: Option<TopologySpec>,
    coordinator_bound: Arc<AtomicBool>,
    revision: Arc<AtomicU32>,
}

impl MobTopologyService {
    pub fn new(spec: Option<TopologySpec>) -> Self {
        Self {
            spec,
            coordinator_bound: Arc::new(AtomicBool::new(false)),
            revision: Arc::new(AtomicU32::new(0)),
        }
    }

    pub fn evaluate(&self, from_role: &ProfileName, to_role: &ProfileName) -> PolicyDecision {
        self.spec.as_ref().map_or(PolicyDecision::Allow, |spec| {
            evaluate_topology(&spec.rules, from_role, to_role)
        })
    }

    pub fn bind_coordinator(&self) -> u32 {
        if !self.coordinator_bound.swap(true, Ordering::AcqRel) {
            return self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        }
        self.revision()
    }

    pub fn unbind_coordinator(&self) -> u32 {
        if self.coordinator_bound.swap(false, Ordering::AcqRel) {
            return self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        }
        self.revision()
    }

    pub fn note_spawn_boundary(&self) -> u32 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn coordinator_bound(&self) -> bool {
        self.coordinator_bound.load(Ordering::Acquire)
    }

    pub fn revision(&self) -> u32 {
        self.revision.load(Ordering::Acquire)
    }
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
        .rfind(|rule| {
            role_matches(&rule.from_role, from_role) && role_matches(&rule.to_role, to_role)
        })
        .map_or(PolicyDecision::Allow, |rule| {
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
    use crate::definition::{PolicyMode, TopologySpec};

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
            PolicyDecision::Deny
        );
        assert_eq!(
            evaluate_topology(
                &rules,
                &ProfileName::from("lead"),
                &ProfileName::from("reviewer")
            ),
            PolicyDecision::Deny
        );
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
    fn topology_service_tracks_coordinator_and_revision() {
        let service = MobTopologyService::new(Some(TopologySpec {
            mode: PolicyMode::Advisory,
            rules: vec![],
        }));
        assert_eq!(service.revision(), 0);
        assert!(!service.coordinator_bound());

        assert_eq!(service.bind_coordinator(), 1);
        assert!(service.coordinator_bound());
        assert_eq!(service.note_spawn_boundary(), 2);
        assert_eq!(service.unbind_coordinator(), 3);
        assert!(!service.coordinator_bound());
    }
}
