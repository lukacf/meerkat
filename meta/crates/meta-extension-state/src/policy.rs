use crate::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Operation {
    Read,
    Edit,
    Manage,
}

fn manages(caller: &HostAccessContext, owner: &Owner) -> bool {
    match owner {
        Owner::Agent(principal) => principal == &caller.principal,
        Owner::Mob(id) => caller.managed_mobs.contains(id),
        Owner::Realm => caller.realm_admin,
    }
}

pub fn can_create(caller: &HostAccessContext, owner: &Owner) -> bool {
    caller.policy.read
        && caller.policy.edit
        && caller.policy.manage
        && caller.owner_is_valid(owner)
        && manages(caller, owner)
}

// A matching route can deny all delegated access and still match a subtree
// deny. Keep that fact distinct from an audience that does not match at all.
struct MatchedRoute {
    ceiling: Option<Role>,
}

fn agent_route(
    caller: &HostAccessContext,
    principal: &Principal,
    reach: Reach,
) -> Option<MatchedRoute> {
    if &caller.principal == principal {
        return Some(MatchedRoute {
            ceiling: Some(Role::Editor),
        });
    }
    if reach == Reach::SelfOnly {
        return None;
    }
    let mut ceiling = Some(Role::Editor);
    for link in &caller.lineage.links {
        if reach == Reach::Forks && link.kind != EdgeKind::Fork {
            return None;
        }
        ceiling = ceiling.zip(link.ceiling).map(|(a, b)| a.min(b));
        if &link.parent == principal {
            return Some(MatchedRoute { ceiling });
        }
    }
    None
}

fn route(caller: &HostAccessContext, audience: &Audience) -> Option<MatchedRoute> {
    match audience {
        Audience::Agent { principal, reach } => agent_route(caller, principal, *reach),
        Audience::Mob(id) => caller.member_mobs.contains(id).then_some(MatchedRoute {
            ceiling: Some(Role::Editor),
        }),
        Audience::Realm => Some(MatchedRoute {
            ceiling: Some(Role::Editor),
        }),
    }
}

/// Single policy evaluator shared by all backends. Invoke against the current
/// record inside the write transaction; authorization precedes revision checks.
pub fn authorize(caller: &HostAccessContext, document: &Document, operation: Operation) -> bool {
    match operation {
        Operation::Read if !caller.policy.read => return false,
        Operation::Edit if !caller.policy.read || !caller.policy.edit => return false,
        Operation::Manage => return caller.policy.manage && manages(caller, &document.owner),
        _ => {}
    }
    if document.access.denies.iter().any(|deny| {
        route(caller, &deny.audience).is_some()
            && (deny.operation == DeniedOperation::Read || operation == Operation::Edit)
    }) {
        return false;
    }
    let mut role = match &document.owner {
        Owner::Agent(principal) => agent_route(caller, principal, document.access.owner_reach)
            .and_then(|route| route.ceiling),
        owner => manages(caller, owner).then_some(Role::Editor),
    };
    for grant in &document.access.grants {
        if let Some(ceiling) = route(caller, &grant.audience).and_then(|route| route.ceiling) {
            let candidate = grant.role.min(ceiling);
            role = Some(role.map_or(candidate, |old| old.max(candidate)));
        }
    }
    role.is_some_and(|role| operation == Operation::Read || role == Role::Editor)
}
