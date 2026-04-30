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
