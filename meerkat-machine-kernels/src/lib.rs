#![cfg_attr(
    feature = "test-oracle",
    allow(
        clippy::all,
        clippy::bool_comparison,
        clippy::cloned_instead_of_copied,
        clippy::collapsible_else_if,
        clippy::expect_used,
        clippy::explicit_iter_loop,
        clippy::if_not_else,
        clippy::len_zero,
        clippy::overly_complex_bool_expr,
        clippy::partialeq_to_none,
        clippy::ptr_arg,
        clippy::redundant_clone,
        clippy::redundant_closure,
        clippy::uninlined_format_args
    )
)]

#[cfg(feature = "test-oracle")]
pub mod compat_generated;
pub mod generated;
#[cfg(feature = "test-oracle")]
mod legacy_generated;
#[doc(hidden)]
pub mod mob_runtime_generated;
mod runtime;
#[cfg(feature = "test-oracle")]
mod test_oracle_compat_generated;

#[cfg(feature = "test-oracle")]
pub mod test_oracle {
    pub mod compat_generated {
        pub use crate::test_oracle_compat_generated::{flow_frame, flow_run, loop_iteration};
    }

    pub mod legacy_generated {
        pub use crate::legacy_generated::{flow_frame, flow_run, loop_iteration};
    }

    pub use crate::runtime::{
        GeneratedMachineKernel, KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue,
        TransitionOutcome, TransitionRefusal,
    };
}
