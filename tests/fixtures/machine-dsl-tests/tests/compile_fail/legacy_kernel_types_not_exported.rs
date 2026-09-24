use meerkat_machine_kernels::{KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue};

fn main() {
    let _state = KernelState::default();
    let _input = KernelInput {
        variant: String::new(),
        fields: std::collections::BTreeMap::new(),
    };
    let _signal = KernelSignal {
        variant: String::new(),
        fields: std::collections::BTreeMap::new(),
    };
    let _effect = KernelEffect {
        variant: String::new(),
        fields: std::collections::BTreeMap::new(),
    };
    let _value = KernelValue::None;
}
