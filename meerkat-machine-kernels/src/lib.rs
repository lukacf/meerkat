pub mod compat_generated;
pub mod generated;
#[cfg(feature = "test-oracle")]
pub mod legacy_generated;
mod runtime;

#[cfg(feature = "test-oracle")]
pub mod legacy {
    pub use crate::runtime::{
        KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue, TransitionOutcome,
        TransitionRefusal,
    };
}

#[cfg(feature = "test-oracle")]
pub mod test_oracle {
    pub use crate::legacy::{
        KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue, TransitionOutcome,
        TransitionRefusal,
    };
    pub use crate::runtime::GeneratedMachineKernel;
}

pub use runtime::{GeneratedMachineKernel, TransitionOutcome, TransitionRefusal};
