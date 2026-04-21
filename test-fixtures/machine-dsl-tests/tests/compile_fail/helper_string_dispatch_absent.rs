use std::collections::BTreeMap;

use meerkat_machine_kernels::TransitionRefusal;
use meerkat_machine_kernels::legacy::{KernelState, KernelValue};

fn main() {
    let _legacy_dispatch: fn(
        &KernelState,
        &str,
        &BTreeMap<String, KernelValue>,
    ) -> Result<KernelValue, TransitionRefusal> =
        meerkat_machine_kernels::generated::meerkat::evaluate_helper;
}
