#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated")
}

fn generated_sources() -> Vec<(PathBuf, String)> {
    fs::read_dir(generated_dir())
        .expect("generated dir")
        .map(|entry| entry.expect("generated entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rs"))
        .map(|path| {
            let source = fs::read_to_string(&path).expect("generated source");
            (path, source)
        })
        .collect()
}

#[test]
fn typed_composition_modules_do_not_reference_legacy_kernel_types() {
    for (path, source) in generated_sources() {
        for forbidden in [
            "KernelState",
            "KernelInput",
            "KernelEffect",
            "KernelValue",
            ".fields.get(",
            ".phase.as_str()",
            "effect.variant",
            "helper_name: &str",
            "evaluate_helper(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} should not contain `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn schedule_bundle_generated_helpers_are_typed() {
    typed_composition_modules_do_not_reference_legacy_kernel_types();
}

#[test]
fn schedule_runtime_bundle_generated_helpers_are_typed() {
    typed_composition_modules_do_not_reference_legacy_kernel_types();
}
