use super::*;
use meerkat_toolkit::{
    authority::ActionPermission,
    host_lifetime::policy::CompiledLocalPolicy,
};

fn native() -> CompiledNativeHost {
    let (_directory, selection) = crate::installed_host::tests::fixture();
    let policy: CompiledLocalPolicy = serde_json::from_slice(
        &std::fs::read(selection.local_policy.path).unwrap(),
    ).unwrap();
    policy.native_host.unwrap()
}

fn id(value: &str) -> Id { Id::new(value).unwrap() }

fn ceiling() -> Ceiling {
    Ceiling {
        actions: Default::default(),
        services: Default::default(),
        frontends: Default::default(),
        output_audiences: [id("person")].into(),
    }
}

fn permission(action: Action, resource: &str) -> ActionPermission {
    ActionPermission { action, resource: id(resource) }
}

#[test]
fn acquisition_ceiling_requires_delegate_for_same_selected_target() {
    let mut native = native();
    let mut other = native.members[0].clone();
    other.member.member = "other-agent".into();
    native.members.push(other);
    let mut selected = ceiling();
    selected.actions.insert(permission(Action::FindAndInstall, "agent"));
    let before = selected.clone();
    assert!(matches!(validate_proposal_ceiling(&native, &selected),
        Err(HostLifetimeError::InvalidSelection)));
    assert_eq!(selected, before);
    selected.actions.insert(permission(Action::Delegate, "other-agent"));
    let before = selected.clone();
    assert!(matches!(validate_proposal_ceiling(&native, &selected),
        Err(HostLifetimeError::InvalidSelection)));
    assert_eq!(selected, before);
    selected.actions.insert(permission(Action::Delegate, "agent"));
    assert!(validate_proposal_ceiling(&native, &selected).is_ok());
}

#[test]
fn acquisition_ceiling_pair_preserves_native_and_other_proposal_bounds() {
    let native = native();
    let native_before = canonical_json(&native).unwrap();
    let mut selected = ceiling();
    selected.actions.extend([
        permission(Action::FindAndInstall, "agent"),
        permission(Action::Delegate, "agent"),
        permission(Action::Deliver, "existing-output"),
    ]);
    let before = selected.clone();
    assert!(validate_proposal_ceiling(&native, &selected).is_ok());
    assert_eq!(selected, before);
    assert_eq!(canonical_json(&native).unwrap(), native_before);
    assert!(selected.permits(Action::FindAndInstall, &id("agent")));
    assert!(!selected.permits(Action::FindAndInstall, &id("other-agent")));
    assert_eq!(selected.output_audiences,
        std::collections::BTreeSet::from([id("person")]));
}

#[test]
fn non_acquisition_ceiling_does_not_require_delegate() {
    let native = native();
    let mut selected = ceiling();
    assert!(validate_proposal_ceiling(&native, &selected).is_ok());
    selected.actions.insert(permission(Action::Deliver, "existing-output"));
    let before = selected.clone();
    assert!(validate_proposal_ceiling(&native, &selected).is_ok());
    assert_eq!(selected, before);
    assert!(!selected.permits(Action::FindAndInstall, &id("agent")));
    assert!(!selected.permits(Action::Delegate, &id("agent")));
}

#[test]
fn acquisition_ceiling_refuses_unknown_or_ambiguous_selected_target() {
    let mut native = native();
    let mut selected = ceiling();
    selected.actions.extend([
        permission(Action::FindAndInstall, "agent"),
        permission(Action::Delegate, "agent"),
    ]);
    assert!(validate_proposal_ceiling(&native, &selected).is_ok());
    let member = native.members.pop().unwrap();
    assert!(matches!(validate_proposal_ceiling(&native, &selected),
        Err(HostLifetimeError::InvalidSelection)));
    native.members.extend([member.clone(), member]);
    assert!(matches!(validate_proposal_ceiling(&native, &selected),
        Err(HostLifetimeError::InvalidSelection)));
}
