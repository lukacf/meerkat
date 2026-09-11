//! Resource units for the feature-owned Live ledger.
//!
//! Charges describe encoded storage, not decoded text or tokens. These value
//! operations issue no reservation or admission authority; generated Live
//! machines own the corresponding state transitions.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResourceCharge {
    pub records: u64,
    pub encoded_bytes: u64,
}

impl LiveResourceCharge {
    pub fn checked_add(self, other: Self) -> Result<Self, LiveResourceArithmeticError> {
        Ok(Self {
            records: self
                .records
                .checked_add(other.records)
                .ok_or(LiveResourceArithmeticError::Overflow)?,
            encoded_bytes: self
                .encoded_bytes
                .checked_add(other.encoded_bytes)
                .ok_or(LiveResourceArithmeticError::Overflow)?,
        })
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, LiveResourceArithmeticError> {
        Ok(Self {
            records: self
                .records
                .checked_sub(other.records)
                .ok_or(LiveResourceArithmeticError::Underflow)?,
            encoded_bytes: self
                .encoded_bytes
                .checked_sub(other.encoded_bytes)
                .ok_or(LiveResourceArithmeticError::Underflow)?,
        })
    }
}

/// Kind of future terminal/control envelope whose capacity must be covered
/// before an obligation is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveCompletionObligation {
    ChannelControl,
    RequestChain,
    EffectStart,
    FunctionOutput,
    Continuation,
    ContextChunk,
    CallbackContinuation,
}

impl LiveCompletionObligation {
    /// Base product-policy budget. Variable member lists and ordinary input
    /// charges are added from their exact encoding before machine admission.
    ///
    /// These ceilings do not certify an envelope's fit. Representation
    /// contracts and the shared encoder must prove that separately.
    #[must_use]
    pub const fn base_budget(self) -> LiveResourceCharge {
        let (records, kibibytes) = match self {
            Self::ChannelControl => (64, 128),
            Self::RequestChain => (32, 160),
            Self::EffectStart => (32, 96),
            Self::FunctionOutput => (16, 48),
            Self::Continuation => (16, 32),
            Self::ContextChunk => (12, 8),
            Self::CallbackContinuation => (16, 48),
        };
        LiveResourceCharge {
            records,
            encoded_bytes: kibibytes * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveResourceArithmeticError {
    #[error("live resource charge overflows its integer representation")]
    Overflow,
    #[error("live resource charge subtraction exceeds the recorded charge")]
    Underflow,
}
