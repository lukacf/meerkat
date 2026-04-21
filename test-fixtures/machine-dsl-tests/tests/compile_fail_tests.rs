#[test]
fn compile_fail_schema_validation_fixtures() {
    let t = trybuild::TestCases::new();
    for fixture in [
        "tests/compile_fail/derived_phase_missing_projection.rs",
        "tests/compile_fail/duplicate_transition_name.rs",
        "tests/compile_fail/unknown_field_in_guard.rs",
        "tests/compile_fail/unknown_phase_in_terminal.rs",
    ] {
        t.compile_fail(fixture);
    }
}

#[test]
fn compile_fail_row22_public_api_fixtures() {
    let t = trybuild::TestCases::new();
    for fixture in [
        "tests/compile_fail/legacy_kernel_types_not_exported.rs",
        "tests/compile_fail/typed_state_has_no_fields_map.rs",
        "tests/compile_fail/state_phase_as_str_absent.rs",
        "tests/compile_fail/typed_phase_has_no_as_str.rs",
        "tests/compile_fail/helper_string_dispatch_absent.rs",
        "tests/compile_fail/canonical_transition_rejects_string_input_identity.rs",
        "tests/compile_fail/compat_transition_rejects_string_input_identity.rs",
        "tests/compile_fail/canonical_transition_signal_rejects_string_identity.rs",
        "tests/compile_fail/typed_effect_has_no_variant_string.rs",
        "tests/compile_fail/composition_protocol_rejects_raw_effect_shape.rs",
    ] {
        t.compile_fail(fixture);
    }
}
