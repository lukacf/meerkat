//! Static guard for the terminal-publication ownership seam.
//!
//! Runtime-loop exit used to mint a second waiter-only RuntimeTerminated
//! result after the control plane had already terminalized the same inputs.
//! That split directed session events from completion waiters and could also
//! overwrite a more specific stop/reset reason. Keep all runless terminal
//! outcomes routed through the durable control-plane publisher.

use std::fs;
use std::path::Path;

#[test]
fn runtime_loop_exit_has_no_waiter_only_runtime_terminal_fallback() -> std::io::Result<()> {
    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let runtime_loop = fs::read_to_string(crate_root.join("src/runtime_loop.rs"))?;
    let control_plane = fs::read_to_string(crate_root.join("src/control_plane.rs"))?;

    assert!(
        !runtime_loop.contains("resolve_all_runtime_terminated"),
        "runtime-loop exit must not mint a waiter-only terminal; leave stale or unpublished work for durable recovery"
    );
    assert!(
        control_plane.contains("publish_and_resolve_runless_runtime_termination"),
        "runless stop/reset/archive terminalization must retain one durable publication owner"
    );
    assert!(
        control_plane.contains("authorize_runtime_terminal_bundle"),
        "directed events and waiter results must be projected from one generated terminal bundle"
    );

    Ok(())
}
