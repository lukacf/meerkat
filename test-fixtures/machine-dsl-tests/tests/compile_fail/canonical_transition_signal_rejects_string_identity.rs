use std::collections::BTreeMap;

use meerkat_machine_kernels::legacy::KernelSignal;

fn main() {
    let state = meerkat_machine_kernels::generated::meerkat::initial_state();
    let signal = KernelSignal {
        variant: "Initialize".into(),
        fields: BTreeMap::new(),
    };
    let _ = meerkat_machine_kernels::generated::meerkat::transition_signal(&state, &signal);
}
