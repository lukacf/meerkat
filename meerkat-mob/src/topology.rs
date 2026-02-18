//! Topology policy evaluation for inter-meerkat communication.
//!
//! [`TopologyEvaluator`] evaluates communication attempts against the
//! [`TopologySpec`] rules, producing [`TopologyDecision`] outcomes. Two
//! domains are supported:
//!
//! - **flow_dispatched**: Communication initiated by the flow engine during
//!   step dispatch. Governed by `TopologySpec.mode`.
//! - **ad_hoc**: Communication initiated outside of flow dispatch (e.g., direct
//!   peer-to-peer messages). Governed by `TopologySpec.ad_hoc_mode`, which
//!   defaults to advisory even when the flow mode is strict.

use crate::spec::{AdHocMode, TopologyAction, TopologyMode, TopologySpec};

/// The domain of a communication attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyDomain {
    /// Communication dispatched by the flow engine (step dispatch).
    FlowDispatched,
    /// Ad-hoc communication outside of flow dispatch.
    AdHoc,
}

/// The decision made by the topology evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopologyDecision {
    /// Communication is allowed.
    Allow,
    /// Communication is allowed but a topology violation was observed (advisory).
    Warn {
        /// Source role.
        from_role: String,
        /// Destination role.
        to_role: String,
        /// Human-readable description.
        description: String,
    },
    /// Communication is blocked (strict mode).
    Block {
        /// Source role.
        from_role: String,
        /// Destination role.
        to_role: String,
    },
}

/// Evaluates communication attempts against topology rules.
///
/// This is a pure function evaluator (no I/O). Event emission is the
/// responsibility of the caller based on the returned [`TopologyDecision`].
pub struct TopologyEvaluator<'a> {
    spec: &'a TopologySpec,
}

impl<'a> TopologyEvaluator<'a> {
    /// Create a new evaluator for the given topology spec.
    pub fn new(spec: &'a TopologySpec) -> Self {
        Self { spec }
    }

    /// Evaluate whether communication from `from_role` to `to_role` is allowed.
    ///
    /// The `domain` determines which enforcement mode is used:
    /// - [`TopologyDomain::FlowDispatched`]: uses `TopologySpec.mode`
    /// - [`TopologyDomain::AdHoc`]: uses `TopologySpec.ad_hoc_mode` (always
    ///   advisory unless `StrictOptIn`)
    pub fn evaluate(
        &self,
        from_role: &str,
        to_role: &str,
        _intent: &str,
        domain: TopologyDomain,
    ) -> TopologyDecision {
        // Find the first matching rule
        let matched_action = self
            .spec
            .rules
            .iter()
            .find(|rule| rule.from == from_role && rule.to == to_role)
            .map(|rule| rule.action);

        // Resolve the effective action: matched rule action, or default
        let effective_action = matched_action.unwrap_or(self.spec.default_action);

        match effective_action {
            TopologyAction::Allow => TopologyDecision::Allow,
            TopologyAction::Deny => {
                // Determine enforcement mode based on domain
                let is_strict = match domain {
                    TopologyDomain::FlowDispatched => {
                        self.spec.mode == TopologyMode::Strict
                    }
                    TopologyDomain::AdHoc => {
                        // ad_hoc defaults advisory even under strict flow
                        self.spec.ad_hoc_mode == AdHocMode::StrictOptIn
                    }
                };

                if is_strict {
                    TopologyDecision::Block {
                        from_role: from_role.to_string(),
                        to_role: to_role.to_string(),
                    }
                } else {
                    // Advisory: warn but allow
                    TopologyDecision::Warn {
                        from_role: from_role.to_string(),
                        to_role: to_role.to_string(),
                        description: format!(
                            "topology violation (advisory): {} -> {} denied by rule",
                            from_role, to_role
                        ),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{TopologyRule, TopologySpec};

    fn advisory_spec_with_deny_rule() -> TopologySpec {
        TopologySpec {
            mode: TopologyMode::Advisory,
            ad_hoc_mode: AdHocMode::Advisory,
            default_action: TopologyAction::Allow,
            rules: vec![TopologyRule {
                from: "reviewer".to_string(),
                to: "coordinator".to_string(),
                action: TopologyAction::Deny,
            }],
        }
    }

    fn strict_spec_with_deny_rule() -> TopologySpec {
        TopologySpec {
            mode: TopologyMode::Strict,
            ad_hoc_mode: AdHocMode::Advisory, // ad_hoc still advisory
            default_action: TopologyAction::Allow,
            rules: vec![TopologyRule {
                from: "reviewer".to_string(),
                to: "coordinator".to_string(),
                action: TopologyAction::Deny,
            }],
        }
    }

    #[test]
    fn test_topology_advisory_allows_with_warning() {
        let spec = advisory_spec_with_deny_rule();
        let eval = TopologyEvaluator::new(&spec);

        // Denied rule in advisory mode: should warn
        let decision = eval.evaluate(
            "reviewer",
            "coordinator",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        match &decision {
            TopologyDecision::Warn {
                from_role,
                to_role,
                description,
            } => {
                assert_eq!(from_role, "reviewer");
                assert_eq!(to_role, "coordinator");
                assert!(
                    description.contains("advisory"),
                    "Warning should mention advisory: {description}"
                );
            }
            other => panic!("Expected Warn, got: {other:?}"),
        }
    }

    #[test]
    fn test_topology_advisory_allows_non_matching() {
        let spec = advisory_spec_with_deny_rule();
        let eval = TopologyEvaluator::new(&spec);

        // No matching rule, default is Allow
        let decision = eval.evaluate(
            "coordinator",
            "reviewer",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        assert_eq!(decision, TopologyDecision::Allow);
    }

    #[test]
    fn test_topology_strict_blocks() {
        let spec = strict_spec_with_deny_rule();
        let eval = TopologyEvaluator::new(&spec);

        // Denied rule in strict mode: should block
        let decision = eval.evaluate(
            "reviewer",
            "coordinator",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        match &decision {
            TopologyDecision::Block {
                from_role,
                to_role,
            } => {
                assert_eq!(from_role, "reviewer");
                assert_eq!(to_role, "coordinator");
            }
            other => panic!("Expected Block, got: {other:?}"),
        }
    }

    #[test]
    fn test_topology_strict_allows_non_matching() {
        let spec = strict_spec_with_deny_rule();
        let eval = TopologyEvaluator::new(&spec);

        // No matching rule, default is Allow
        let decision = eval.evaluate(
            "coordinator",
            "reviewer",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        assert_eq!(decision, TopologyDecision::Allow);
    }

    #[test]
    fn test_topology_ad_hoc_advisory_under_strict_flow() {
        // Even with strict flow mode, ad_hoc defaults to advisory
        let spec = strict_spec_with_deny_rule();
        let eval = TopologyEvaluator::new(&spec);

        let decision = eval.evaluate(
            "reviewer",
            "coordinator",
            "delegate",
            TopologyDomain::AdHoc,
        );
        // ad_hoc_mode is Advisory, so deny rule produces Warn, not Block
        match &decision {
            TopologyDecision::Warn { .. } => {}
            other => panic!(
                "Expected Warn for ad_hoc under strict flow, got: {other:?}"
            ),
        }
    }

    #[test]
    fn test_topology_ad_hoc_strict_opt_in_blocks() {
        let spec = TopologySpec {
            mode: TopologyMode::Strict,
            ad_hoc_mode: AdHocMode::StrictOptIn,
            default_action: TopologyAction::Allow,
            rules: vec![TopologyRule {
                from: "reviewer".to_string(),
                to: "coordinator".to_string(),
                action: TopologyAction::Deny,
            }],
        };
        let eval = TopologyEvaluator::new(&spec);

        let decision = eval.evaluate(
            "reviewer",
            "coordinator",
            "delegate",
            TopologyDomain::AdHoc,
        );
        match &decision {
            TopologyDecision::Block { .. } => {}
            other => panic!(
                "Expected Block for ad_hoc StrictOptIn, got: {other:?}"
            ),
        }
    }

    #[test]
    fn test_topology_default_deny() {
        let spec = TopologySpec {
            mode: TopologyMode::Strict,
            ad_hoc_mode: AdHocMode::Advisory,
            default_action: TopologyAction::Deny,
            rules: vec![
                // Explicit allow rule
                TopologyRule {
                    from: "coordinator".to_string(),
                    to: "reviewer".to_string(),
                    action: TopologyAction::Allow,
                },
            ],
        };
        let eval = TopologyEvaluator::new(&spec);

        // Explicit allow rule: should allow
        let allowed = eval.evaluate(
            "coordinator",
            "reviewer",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        assert_eq!(allowed, TopologyDecision::Allow);

        // No matching rule, default is Deny, strict mode: should block
        let blocked = eval.evaluate(
            "reviewer",
            "coordinator",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        match &blocked {
            TopologyDecision::Block { .. } => {}
            other => panic!("Expected Block for default deny, got: {other:?}"),
        }
    }

    #[test]
    fn test_topology_explicit_allow_rule() {
        let spec = TopologySpec {
            mode: TopologyMode::Strict,
            ad_hoc_mode: AdHocMode::Advisory,
            default_action: TopologyAction::Deny,
            rules: vec![TopologyRule {
                from: "worker".to_string(),
                to: "manager".to_string(),
                action: TopologyAction::Allow,
            }],
        };
        let eval = TopologyEvaluator::new(&spec);

        let decision = eval.evaluate(
            "worker",
            "manager",
            "delegate",
            TopologyDomain::FlowDispatched,
        );
        assert_eq!(decision, TopologyDecision::Allow);
    }
}
