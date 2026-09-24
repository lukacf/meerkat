use meerkat_machine_kernels::generated::meerkat;

fn main() {
    let state = meerkat::initial_state();
    let _ = meerkat::evaluate_helper(
        &state,
        "StepStatusIs",
        &std::collections::BTreeMap::new(),
    );
}
