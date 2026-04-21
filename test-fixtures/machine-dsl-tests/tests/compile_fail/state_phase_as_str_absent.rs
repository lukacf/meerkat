fn main() {
    let state = meerkat_machine_kernels::generated::meerkat::initial_state();
    let _ = state.phase.as_str();
}
