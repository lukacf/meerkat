use std::collections::BTreeMap;

use meerkat_machine_kernels::legacy::KernelInput;

fn main() {
    let state = meerkat_machine_kernels::generated::meerkat::initial_state();
    let input = KernelInput {
        variant: "PrepareBindings".into(),
        fields: BTreeMap::new(),
    };
    let _ = meerkat_machine_kernels::generated::meerkat::transition(&state, &input);
}
