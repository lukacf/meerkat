#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

fn generated_mod() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated/mod.rs")
}

#[test]
fn public_generated_surface_returns_to_canonical_five() {
    let source = fs::read_to_string(generated_mod()).expect("generated mod");
    assert!(
        source.contains("pub(crate) mod flow_frame_loop_driver;"),
        "mob-local generated surface should keep flow_frame_loop_driver crate-private"
    );
    assert!(
        source.contains("pub mod protocol_flow_loop_until_evaluation;"),
        "mob-local generated protocol helper should stay public while the driver stays crate-private"
    );
}

#[test]
fn flow_runtime_driver_does_not_route_through_compact_bridge_paths() {
    let source = fs::read_to_string(generated_mod()).expect("generated mod");
    for forbidden in ["compact/", "compact_", "CompactBridge", "compact bridge"] {
        assert!(
            !source.contains(forbidden),
            "public generated surface should not normalize a compact bridge via `{forbidden}`"
        );
    }
}
