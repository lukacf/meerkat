fn main() {
    let builder_session_id = meerkat_core::SessionId::new();
    let options_session_id = meerkat_core::SessionId::new();

    let _builder = meerkat::AgentBuilder::new().runtime_build_mode(
        meerkat_core::RuntimeBuildMode::SessionOwned(forged_bindings(builder_session_id)),
    );

    let mut options = meerkat_core::SessionBuildOptions::default();
    options.runtime_build_mode =
        meerkat_core::RuntimeBuildMode::SessionOwned(forged_bindings(options_session_id));
}

fn forged_bindings(session_id: meerkat_core::SessionId) -> meerkat_core::SessionRuntimeBindings {
    meerkat_core::SessionRuntimeBindings {
        session_id,
        epoch_id: meerkat_core::RuntimeEpochId::new(),
        ops_lifecycle: unreachable!("compile-only fixture should never run"),
        cursor_state: unreachable!("compile-only fixture should never run"),
        tool_visibility_owner: unreachable!("compile-only fixture should never run"),
        turn_state: unreachable!("compile-only fixture should never run"),
        comms_drain: unreachable!("compile-only fixture should never run"),
        external_tool_surface: unreachable!("compile-only fixture should never run"),
        peer_comms: unreachable!("compile-only fixture should never run"),
        session_admission: unreachable!("compile-only fixture should never run"),
        model_routing: unreachable!("compile-only fixture should never run"),
        auth_lease: unreachable!("compile-only fixture should never run"),
        mcp_server_lifecycle: unreachable!("compile-only fixture should never run"),
        peer_interaction: unreachable!("compile-only fixture should never run"),
        session_context: unreachable!("compile-only fixture should never run"),
        session_claim_handle: unreachable!("compile-only fixture should never run"),
        interaction_stream: unreachable!("compile-only fixture should never run"),
        realtime_product_turn: unreachable!("compile-only fixture should never run"),
    }
}
