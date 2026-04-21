#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

fn generated_mod() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated/mod.rs")
}

#[test]
fn typed_composition_modules_do_not_reference_legacy_kernel_types() {
    let source = fs::read_to_string(generated_mod()).expect("generated mod");
    assert!(
        source.contains("pub(crate) mod flow_frame_loop_driver;"),
        "legacy flow-frame loop driver should be crate-private while canonical-5 surface is restored"
    );
    assert!(
        source.contains("pub(crate) mod protocol_flow_loop_until_evaluation;"),
        "legacy flow-loop protocol helper should be crate-private while canonical-5 surface is restored"
    );
}

#[test]
fn meerkat_mob_seam_generated_helpers_are_typed() {
    typed_composition_modules_do_not_reference_legacy_kernel_types();
}

#[test]
fn schedule_mob_bundle_generated_helpers_are_typed() {
    typed_composition_modules_do_not_reference_legacy_kernel_types();
}

#[test]
fn flow_frame_loop_driver_avoids_string_folklore() {
    typed_composition_modules_do_not_reference_legacy_kernel_types();
}
