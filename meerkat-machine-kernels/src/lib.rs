pub mod generated;
#[cfg(feature = "test-oracle")]
mod runtime;
#[allow(dead_code)]
pub(crate) mod ids {
    use serde::{Deserialize, Serialize};
    use std::fmt;

    macro_rules! string_id {
        ($name:ident) => {
            #[derive(
                Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
            )]
            pub struct $name(pub String);
            impl<T: Into<String>> From<T> for $name {
                fn from(value: T) -> Self {
                    Self(value.into())
                }
            }
            impl $name {
                pub fn as_str(&self) -> &str {
                    &self.0
                }
            }
            impl fmt::Display for $name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str(&self.0)
                }
            }
        };
    }

    string_id!(AgentIdentity);
    string_id!(AgentRuntimeId);
    string_id!(MobId);
    string_id!(SessionId);
    string_id!(WorkRef);
    string_id!(RunId);
    string_id!(FrameId);
    string_id!(LoopInstanceId);
    string_id!(LoopId);
    string_id!(FlowNodeId);
    string_id!(BranchId);
    string_id!(StepId);
    string_id!(ToolFilter);
    string_id!(ToolVisibilityWitness);

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct FenceToken(pub u64);
    impl FenceToken {
        pub const fn get(self) -> u64 {
            self.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct Generation(pub u64);
    impl Generation {
        pub const fn get(self) -> u64 {
            self.0
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum WorkOrigin {
        External,
        Internal,
    }
}

#[cfg(feature = "test-oracle")]
pub mod test_oracle {
    pub use crate::runtime::{
        GeneratedMachineKernel, KernelEffect, KernelInput, KernelSignal, KernelState, KernelValue,
        TransitionOutcome, TransitionRefusal,
    };
}
