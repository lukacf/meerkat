//! Surface declaration contracts, not provider execution qualification.

use meerkat_contracts::{RpcMethodCatalogOptions, rpc_method_catalog};
use serde_json::json;

#[test]
fn local_transport_and_placed_member_control_declarations_are_independent() {
    let mut options = RpcMethodCatalogOptions::documented_surface();
    options.live_enabled = false;
    options.live_webrtc_enabled = false;
    let without_local_listener = rpc_method_catalog(options);
    assert!(
        !without_local_listener
            .iter()
            .any(|method| method.name == "live/open")
    );
    assert!(
        without_local_listener
            .iter()
            .any(|method| method.name == "mob/member_live_open")
    );
    options.live_enabled = true;
    let ws = rpc_method_catalog(options);
    assert!(
        ws.iter().any(
            |method| method.name == "live/open" && method.params_type == Some("LiveOpenParams")
        )
    );
    assert!(!ws.iter().any(|method| method.name == "live/webrtc/answer"));
    options.live_webrtc_enabled = true;
    assert!(
        rpc_method_catalog(options)
            .iter()
            .any(|method| method.name == "live/webrtc/answer")
    );
}

#[test]
fn rest_has_no_public_live_mutation_contract() {
    for path in meerkat_contracts::rest_path_catalog() {
        for operation in path.operations {
            assert!(!matches!(
                operation.request_schema,
                Some("LiveOpenParams" | "MobMemberLiveOpenParams")
            ));
            assert!(!path.path.contains("/live") || operation.method == "get");
        }
    }
}

#[tokio::test]
async fn public_mcp_does_not_advertise_or_dispatch_live_control() {
    let names = meerkat_mob_mcp::public_tool_names();
    let state = meerkat_mob_mcp::MobMcpState::new_in_memory();
    for name in [
        "meerkat_mob_member_live_open",
        "meerkat_mob_member_live_close",
        "meerkat_mob_member_live_control",
        "meerkat_live_open",
    ] {
        assert!(!names.contains(&name));
        assert!(
            meerkat_mob_mcp::handle_public_tools_call(
                &state,
                name,
                &json!({"mob_id":"mob","agent_identity":"member","profile_id":"voice"}),
            )
            .await
            .is_err()
        );
    }
}

#[test]
fn member_selector_carries_no_controller_credentials_or_permission()
-> Result<(), Box<dyn std::error::Error>> {
    use meerkat_contracts::MobMemberLiveOpenParams;
    let value = json!({"mob_id":"mob","agent_identity":"member","profile_id":"voice","turning_mode":"continuous"});
    let params: MobMemberLiveOpenParams = serde_json::from_value(value.clone())?;
    assert_eq!(serde_json::to_value(params)?, value);
    for field in ["auth_binding", "api_key", "grant", "session_id", "run_id"] {
        let mut forbidden = value.clone();
        forbidden[field] = json!("controller-must-not-author-this");
        assert!(serde_json::from_value::<MobMemberLiveOpenParams>(forbidden).is_err());
    }
    Ok(())
}
