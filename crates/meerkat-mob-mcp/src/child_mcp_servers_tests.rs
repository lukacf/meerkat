#![allow(clippy::unwrap_used)]
use super::*;
use serde_json::json;

fn descriptor(name: &str, endpoint: &str) -> McpServerConfig {
    McpServerConfig::streamable_http(name, endpoint, Default::default())
}
fn definition() -> MobDefinition {
    serde_json::from_value(json!({"id":"child", "profiles":{
        "worker":{"model":"test-model"},
        "delegate":{"model":"test-model"},
        "host":{"realm_profile":"host-profile"}
    }}))
    .unwrap()
}

#[test]
fn child_supply_is_explicit_idempotent_and_persisted() {
    let public = descriptor("shared", "http://127.0.0.1/public-binding");
    let supply = ChildMcpServers::new()
        .register(public.clone(), ChildToolBundleAvailability::ChildAvailable)
        .unwrap()
        .register(
            descriptor("private", "http://127.0.0.1/private-binding"),
            ChildToolBundleAvailability::HostOnly,
        )
        .unwrap();
    let mut child = definition();
    supply.supply(&mut child).unwrap();
    supply.supply(&mut child).unwrap();
    for profile in child
        .profiles
        .values()
        .filter_map(|binding| binding.as_inline())
    {
        assert_eq!(profile.tools.mcp_servers, vec![public.clone()]);
    }
    let persisted = serde_json::to_vec(&child).unwrap();
    let restored: MobDefinition = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(restored, child);
    assert!(
        !String::from_utf8(persisted)
            .unwrap()
            .contains("private-binding")
    );
}

#[test]
fn conflicting_registration_and_profile_binding_refuse_without_replacement() {
    let public = descriptor("shared", "http://127.0.0.1/public-binding");
    let changed = descriptor("shared", "http://127.0.0.1/different-binding");
    let supply = ChildMcpServers::new()
        .register(public.clone(), ChildToolBundleAvailability::ChildAvailable)
        .unwrap();
    assert!(
        supply
            .clone()
            .register(changed.clone(), ChildToolBundleAvailability::ChildAvailable)
            .is_err()
    );
    let mut child = definition();
    child
        .profiles
        .values_mut()
        .filter_map(|binding| binding.as_inline_mut())
        .last()
        .unwrap()
        .tools
        .mcp_servers
        .push(changed);
    let before = child.clone();
    assert!(supply.supply(&mut child).is_err());
    assert_eq!(child, before);
    let withdrawn = supply
        .register(public, ChildToolBundleAvailability::HostOnly)
        .unwrap();
    let mut child = definition();
    withdrawn.supply(&mut child).unwrap();
    assert!(
        child
            .profiles
            .values()
            .filter_map(|binding| binding.as_inline())
            .all(|profile| profile.tools.mcp_servers.is_empty())
    );
}
