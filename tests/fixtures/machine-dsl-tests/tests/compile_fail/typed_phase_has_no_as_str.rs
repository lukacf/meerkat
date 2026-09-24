use meerkat_machine_kernels::generated::meerkat;

fn main() {
    let state = meerkat::initial_state();
    let _ = state.phase.as_str();
}
