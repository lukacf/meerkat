use crate::model::TopologyDomainSpec;

pub(crate) fn evaluate_topology(
    domain: &TopologyDomainSpec,
    from_role: &str,
    to_role: &str,
    kind: &str,
    intent: &str,
) -> bool {
    if domain.rules.is_empty() {
        return true;
    }

    for rule in &domain.rules {
        let from_ok = rule.from_roles.is_empty()
            || rule.from_roles.iter().any(|value| value == "*" || value == from_role);
        let to_ok = rule.to_roles.is_empty()
            || rule.to_roles.iter().any(|value| value == "*" || value == to_role);
        let kind_ok = rule.kinds.is_empty()
            || rule.kinds.iter().any(|value| value == "*" || value == kind);
        let intent_ok = rule.intents.is_empty()
            || rule.intents.iter().any(|value| value == "*" || value == intent);

        if from_ok && to_ok && kind_ok && intent_ok {
            return true;
        }
    }

    false
}

pub(crate) fn role_pair_allowed(domain: &TopologyDomainSpec, from_role: &str, to_role: &str) -> bool {
    if domain.rules.is_empty() {
        return true;
    }

    domain.rules.iter().any(|rule| {
        let from_ok = rule.from_roles.is_empty()
            || rule.from_roles.iter().any(|value| value == "*" || value == from_role);
        let to_ok = rule.to_roles.is_empty()
            || rule.to_roles.iter().any(|value| value == "*" || value == to_role);
        from_ok && to_ok
    })
}
