use std::collections::BTreeMap;

use meerkat_machine_kernels::{KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue};

fn main() {
    let _ = KernelState::default();
    let _ = KernelInput {
        variant: "PrepareBindings".into(),
        fields: BTreeMap::new(),
    };
    let _ = KernelSignal {
        variant: "Initialize".into(),
        fields: BTreeMap::new(),
    };
    let _ = KernelEffect {
        variant: "RuntimeBound".into(),
        fields: BTreeMap::new(),
    };
    let _ = KernelValue::None;
}
