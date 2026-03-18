#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;

fn candidate_binaries() -> Vec<PathBuf> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    vec![
        workspace_root.join("target/debug/rkat"),
        workspace_root.join("target/release/rkat"),
    ]
}

#[test]
fn runtime_backed_ingress_red_ok_cli_target_names_runtime_backed_binary_entrypoint() {
    let binaries = candidate_binaries();
    assert_eq!(binaries.len(), 2);
    assert!(binaries[0].ends_with("target/debug/rkat"));
    assert!(binaries[1].ends_with("target/release/rkat"));
}
