use meerkat_machine_kernels::generated::meerkat;

fn main() {
    let state = meerkat::initial_state();
    let _ = meerkat::transition(&state, "RegisterSession".to_string(), &meerkat::EmptyContext);
}
