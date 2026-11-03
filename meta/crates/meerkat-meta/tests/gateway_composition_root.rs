#![allow(clippy::panic)]

//! Structural guard for simplification item 10. The compatibility binaries
//! keep distinct wire/config adapters, but neither may reacquire runtime
//! bootstrap, HTTP admission, or shutdown ownership.

const CONSOLE_GATEWAY: &str = include_str!("../src/bin/meta_gateway.rs");
/// The stdin-RPC gateway lives in the library (`meerkat_meta::rpc_gateway`);
/// its binary only delegates to it.
const STDIO_GATEWAY: &str = include_str!("../src/rpc_gateway.rs");
const STDIO_GATEWAY_BINARY: &str = include_str!("../src/bin/rpc_gateway.rs");

/// Every step appears in `source`, each after the previous one.
fn assert_in_order(name: &str, source: &str, steps: &[&str]) {
    let mut at = 0;
    for step in steps {
        let found = source[at..]
            .find(step)
            .unwrap_or_else(|| panic!("{name} lost ordered shutdown step `{step}`"));
        at += found + step.len();
    }
}

#[test]
fn both_binaries_delegate_runtime_and_http_lifecycle_to_the_library_root() {
    for (name, source) in [
        ("meta_gateway", CONSOLE_GATEWAY),
        ("rpc_gateway", STDIO_GATEWAY),
    ] {
        assert!(
            source.contains("GatewayComposition::prepare"),
            "{name} bypassed the shared typed composition root"
        );
        assert!(
            source.contains("GatewayHttpBinding::bind("),
            "{name} bypassed shared HTTP admission"
        );
        assert!(
            source.contains("validate_http_bind_policy("),
            "{name} binds without the shared HTTP exposure gate"
        );
        assert!(
            source.contains("warn_on_non_loopback_bind("),
            "{name} can bind non-loopback without the shared warning"
        );
        assert!(
            !source.contains("TcpListener::bind("),
            "{name} opened a listener outside GatewayHttpBinding"
        );
        assert!(
            !source.contains("Box::pin(UnifiedRuntime::bootstrap("),
            "{name} reacquired a direct bootstrap root"
        );
        assert!(
            !source.contains("Box::pin(UnifiedRuntime::bootstrap_with_options("),
            "{name} reacquired a direct bootstrap-with-options root"
        );
        assert!(
            !source.contains("axum::serve("),
            "{name} reacquired direct HTTP lifecycle ownership"
        );
    }
}

#[test]
fn both_gateways_shut_down_in_cooperative_order() {
    // The console gateway hands its whole tail to the composition root.
    assert!(
        CONSOLE_GATEWAY.contains("let shutdown = composition"),
        "meta_gateway bypassed ordered cooperative shutdown"
    );
    // The stdin-RPC gateway drives the same order through its native host
    // seam: close host admission, drain HTTP, stop the composition's event
    // drain, shut the runtime down through the shared root, then join the
    // host's close.
    assert_in_order(
        "rpc_gateway",
        STDIO_GATEWAY,
        &[
            "let mut host_begin = match (self.host_begin.take(), caller_begin)",
            "server.shutdown().await",
            "active.stop_event_drain().await",
            "crate::gateway_composition::shutdown_gateway_runtime(runtime).await",
            "host.join_close().await",
        ],
    );
}

#[test]
fn the_stdio_binary_only_delegates_to_the_library_gateway() {
    assert!(STDIO_GATEWAY_BINARY.contains("meerkat_meta::rpc_gateway::main()"));
    for owned in [
        "GatewayComposition",
        "UnifiedRuntime",
        "GatewayHttpBinding",
        "TcpListener",
        "axum::serve(",
    ] {
        assert!(
            !STDIO_GATEWAY_BINARY.contains(owned),
            "the rpc_gateway binary reacquired `{owned}`"
        );
    }
}

#[test]
fn compatibility_only_command_surfaces_remain_on_their_original_binary() {
    assert!(CONSOLE_GATEWAY.contains("storage-migrate"));
    assert!(CONSOLE_GATEWAY.contains("storage-prune"));
    assert!(!CONSOLE_GATEWAY.contains("META_RPC_REQUEST"));

    assert!(STDIO_GATEWAY.contains("META_RPC_REQUEST"));
    assert!(STDIO_GATEWAY.contains("run_single_shot"));
    assert!(!STDIO_GATEWAY.contains("storage-migrate"));
    assert!(!STDIO_GATEWAY.contains("storage-prune"));
}
