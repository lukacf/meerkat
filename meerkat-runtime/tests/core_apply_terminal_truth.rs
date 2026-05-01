//! Keep core apply terminal state behind one authority.
//!
//! `CoreApplyOutput` may carry receipts and snapshots alongside terminal
//! state, but the terminal fact itself must not be duplicated as both a legacy
//! `run_result` mirror and `CoreApplyTerminal`.

use std::fs;
use std::path::Path;

fn workspace_root() -> Result<&'static Path, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "meerkat-runtime crate should live below workspace root".to_string())
}

fn extract_braced_item<'a>(contents: &'a str, marker: &str) -> Result<&'a str, String> {
    let start = contents
        .find(marker)
        .ok_or_else(|| format!("missing marker `{marker}`"))?;
    let open = contents[start..]
        .find('{')
        .map(|offset| start + offset)
        .ok_or_else(|| format!("missing opening brace after `{marker}`"))?;

    let mut depth = 0usize;
    for (offset, ch) in contents[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&contents[start..=open + offset]);
                }
            }
            _ => {}
        }
    }

    Err(format!("unterminated braced item after `{marker}`"))
}

fn assert_terminal_intent_validation_precedes_markers(
    source: &str,
    owner: &str,
    markers: &[&str],
) -> Result<(), String> {
    let validation = source
        .find("primitive.peer_response_terminal_apply_intent_violation()")
        .ok_or_else(|| {
            format!(
                "{owner} must reject malformed terminal peer-response intent before applying it"
            )
        })?;
    for consumption in markers {
        let consumption = source
            .find(consumption)
            .ok_or_else(|| format!("{owner} missing `{consumption}`"))?;
        if validation >= consumption {
            return Err(format!(
                "{owner} must validate terminal peer-response intent before `{consumption}`"
            ));
        }
    }
    Ok(())
}

fn assert_terminal_context_and_run_stages_pre_turn_appends(source: &str, owner: &str) {
    assert!(
        source.contains("primitive.is_peer_response_terminal_context_and_run()")
            && source.contains("pending_system_context_appends(&staged.context_appends)")
            && source.contains("pre_turn_context_appends"),
        "{owner} must stage terminal context-and-run context into the admitted turn request"
    );
}

#[test]
fn core_apply_terminal_truth_has_one_authority() -> Result<(), String> {
    let root = workspace_root()?;
    let core_executor =
        fs::read_to_string(root.join("meerkat-core/src/lifecycle/core_executor.rs"))
            .map_err(|err| format!("read core executor source: {err}"))?;
    let runtime_loop = fs::read_to_string(root.join("meerkat-runtime/src/runtime_loop.rs"))
        .map_err(|err| format!("read runtime loop source: {err}"))?;

    let output_struct = extract_braced_item(&core_executor, "pub struct CoreApplyOutput")?;
    assert!(
        output_struct.contains("pub terminal: Option<CoreApplyTerminal>"),
        "CoreApplyOutput should expose CoreApplyTerminal as the canonical terminal authority"
    );
    assert!(
        !output_struct.contains("pub run_result:"),
        "CoreApplyOutput must not duplicate terminal truth with a run_result mirror"
    );

    let waiter_resolver = extract_braced_item(&runtime_loop, "fn resolve_completion_waiters")?;
    assert!(
        !waiter_resolver.contains("run_result:"),
        "runtime completion resolution must branch from CoreApplyTerminal only"
    );
    assert!(
        !waiter_resolver.contains("if let Some(result) = run_result"),
        "runtime completion resolution must not keep a separate run_result branch"
    );
    Ok(())
}

#[test]
fn terminal_context_and_run_adapters_use_canonical_primitive_intent() -> Result<(), String> {
    let root = workspace_root()?;
    let runtime_backed = fs::read_to_string(root.join("meerkat/src/surface/runtime_backed.rs"))
        .map_err(|err| format!("read runtime-backed surface source: {err}"))?;
    let mcp_runtime_ingress =
        fs::read_to_string(root.join("meerkat-mcp-server/src/runtime_ingress.rs"))
            .map_err(|err| format!("read MCP runtime ingress source: {err}"))?;

    let runtime_backed_apply = extract_braced_item(&runtime_backed, "async fn apply")?;
    assert!(
        runtime_backed_apply.contains("primitive.is_context_only_apply_without_turn()"),
        "runtime-backed context shortcut must use the canonical primitive intent helper"
    );
    assert_terminal_intent_validation_precedes_markers(
        runtime_backed_apply,
        "runtime-backed apply",
        &[
            "primitive.is_context_only_apply_without_turn()",
            "start_turn_request_from_primitive(&primitive)",
        ],
    )?;
    assert!(
        runtime_backed_apply.contains("start_turn_request_from_primitive(&primitive)")
            && runtime_backed_apply.contains(".apply_runtime_turn("),
        "runtime-backed apply must build one admitted turn request before applying the reaction turn"
    );

    let runtime_backed_request =
        extract_braced_item(&runtime_backed, "fn start_turn_request_from_primitive")?;
    assert_terminal_context_and_run_stages_pre_turn_appends(
        runtime_backed_request,
        "runtime-backed turn request builder",
    );

    let mcp_runtime_apply =
        extract_braced_item(&mcp_runtime_ingress, "async fn apply_runtime_turn")?;
    assert!(
        mcp_runtime_apply.contains("primitive.is_context_only_apply_without_turn()"),
        "MCP runtime ingress must not re-derive context-only terminal behavior from append shape"
    );
    assert_terminal_intent_validation_precedes_markers(
        mcp_runtime_apply,
        "MCP runtime ingress apply",
        &[
            "primitive.is_context_only_apply_without_turn()",
            "let pre_turn_context_appends = match primitive",
        ],
    )?;
    assert_terminal_context_and_run_stages_pre_turn_appends(
        mcp_runtime_apply,
        "MCP runtime ingress apply",
    );
    Ok(())
}
