//! Keep core apply terminal state behind one authority.
//!
//! `CoreApplyOutput` may carry receipts and snapshots alongside terminal
//! state, but the terminal fact itself must not be duplicated as both a legacy
//! `run_result` mirror and `CoreApplyTerminal`.

use std::fs;
use std::path::Path;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("meerkat-runtime crate should live below workspace root")
}

fn extract_braced_item<'a>(contents: &'a str, marker: &str) -> &'a str {
    let start = contents
        .find(marker)
        .unwrap_or_else(|| panic!("missing marker `{marker}`"));
    let open = contents[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("missing opening brace after `{marker}`"));

    let mut depth = 0usize;
    for (offset, ch) in contents[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &contents[start..=open + offset];
                }
            }
            _ => {}
        }
    }

    panic!("unterminated braced item after `{marker}`");
}

fn assert_terminal_intent_validation_precedes_consumption(source: &str, owner: &str) {
    let validation = source
        .find("primitive.peer_response_terminal_apply_intent_violation()")
        .unwrap_or_else(|| {
            panic!("{owner} must reject malformed terminal peer-response intent before applying it")
        });
    for consumption in [
        "primitive.is_context_only_apply_without_turn()",
        "primitive.is_peer_response_terminal_context_and_run()",
    ] {
        let consumption = source
            .find(consumption)
            .unwrap_or_else(|| panic!("{owner} missing `{consumption}`"));
        assert!(
            validation < consumption,
            "{owner} must validate terminal peer-response intent before `{consumption}`"
        );
    }
}

#[test]
fn core_apply_terminal_truth_has_one_authority() {
    let root = workspace_root();
    let core_executor =
        fs::read_to_string(root.join("meerkat-core/src/lifecycle/core_executor.rs"))
            .expect("read core executor source");
    let runtime_loop = fs::read_to_string(root.join("meerkat-runtime/src/runtime_loop.rs"))
        .expect("read runtime loop source");

    let output_struct = extract_braced_item(&core_executor, "pub struct CoreApplyOutput");
    assert!(
        output_struct.contains("pub terminal: Option<CoreApplyTerminal>"),
        "CoreApplyOutput should expose CoreApplyTerminal as the canonical terminal authority"
    );
    assert!(
        !output_struct.contains("pub run_result:"),
        "CoreApplyOutput must not duplicate terminal truth with a run_result mirror"
    );

    let waiter_resolver = extract_braced_item(&runtime_loop, "fn resolve_completion_waiters");
    assert!(
        !waiter_resolver.contains("run_result:"),
        "runtime completion resolution must branch from CoreApplyTerminal only"
    );
    assert!(
        !waiter_resolver.contains("if let Some(result) = run_result"),
        "runtime completion resolution must not keep a separate run_result branch"
    );
}

#[test]
fn terminal_context_and_run_adapters_use_canonical_primitive_intent() {
    let root = workspace_root();
    let runtime_backed = fs::read_to_string(root.join("meerkat/src/surface/runtime_backed.rs"))
        .expect("read runtime-backed surface source");
    let mcp_runtime_ingress =
        fs::read_to_string(root.join("meerkat-mcp-server/src/runtime_ingress.rs"))
            .expect("read MCP runtime ingress source");

    let runtime_backed_apply = extract_braced_item(&runtime_backed, "async fn apply");
    assert!(
        runtime_backed_apply.contains("primitive.is_context_only_apply_without_turn()"),
        "runtime-backed context shortcut must use the canonical primitive intent helper"
    );
    assert_terminal_intent_validation_precedes_consumption(
        runtime_backed_apply,
        "runtime-backed apply",
    );

    assert!(
        runtime_backed_apply.contains("primitive.is_peer_response_terminal_context_and_run()")
            && runtime_backed_apply.contains("apply_runtime_system_context_for_turn"),
        "runtime-backed terminal context-and-run apply must append context before the reaction turn"
    );

    let mcp_runtime_apply =
        extract_braced_item(&mcp_runtime_ingress, "async fn apply_runtime_turn");
    assert!(
        mcp_runtime_apply.contains("primitive.is_context_only_apply_without_turn()"),
        "MCP runtime ingress must not re-derive context-only terminal behavior from append shape"
    );
    assert_terminal_intent_validation_precedes_consumption(
        mcp_runtime_apply,
        "MCP runtime ingress apply",
    );
    assert!(
        mcp_runtime_apply.contains("primitive.is_peer_response_terminal_context_and_run()")
            && mcp_runtime_apply.contains("apply_runtime_system_context_for_turn"),
        "MCP runtime ingress must append terminal context-and-run context before the reaction turn"
    );
}
