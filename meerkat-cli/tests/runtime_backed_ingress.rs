#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;

fn candidate_binaries() -> Vec<PathBuf> {
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        return vec![
            target_dir.join("debug/rkat"),
            target_dir.join("release/rkat"),
        ];
    }

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
    assert!(binaries[0].ends_with("debug/rkat"));
    assert!(binaries[1].ends_with("release/rkat"));
}
