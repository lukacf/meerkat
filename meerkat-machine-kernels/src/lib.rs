pub mod compat_generated;
pub mod generated;
#[cfg(feature = "test-oracle")]
mod legacy_generated;
mod runtime;

#[cfg(feature = "test-oracle")]
pub mod test_oracle {
    pub mod legacy_generated {
        pub use crate::legacy_generated::{flow_frame, flow_run, loop_iteration};
    }

    pub use crate::runtime::{
        GeneratedMachineKernel, KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue,
        TransitionOutcome, TransitionRefusal,
    };
}
