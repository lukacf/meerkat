//! Resource units for the feature-owned Live ledger.
//!
//! Charges describe encoded storage, not decoded text or tokens. These value
//! operations issue no reservation or admission authority; generated Live
//! machines own the corresponding state transitions.

use serde::{Deserialize, Serialize};

/// Per-record logical allowance for secondary storage keys and bookkeeping:
/// session UUID, maximum channel key, sequence, SHA-256 and row/index custody.
/// The remaining charge is the exact bytes produced by the shared codec.
pub const LIVE_RECORD_STORAGE_ALLOWANCE_BYTES: u64 = 36 + 128 + 8 + 32 + 128;
pub const LIVE_EVENT_PREFIX_WITNESS_BYTES: u64 = 8 + 32;
pub const LIVE_EVENT_STORAGE_ALLOWANCE_BYTES: u64 =
    LIVE_RECORD_STORAGE_ALLOWANCE_BYTES + LIVE_EVENT_PREFIX_WITNESS_BYTES;

/// Absolute Live component ceiling; a resolved grant may impose a lower quota.
pub const LIVE_LEDGER_MAX_CHARGE: LiveResourceCharge = LiveResourceCharge {
    records: 1_000_000,
    encoded_bytes: 256 * 1024 * 1024,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResourceCharge {
    pub records: u64,
    pub encoded_bytes: u64,
}

impl LiveResourceCharge {
    pub fn for_event_record(bytes: &[u8]) -> Result<Self, LiveResourceArithmeticError> {
        Self::for_encoded_record(bytes)?.checked_add(Self {
            records: 0,
            encoded_bytes: LIVE_EVENT_PREFIX_WITNESS_BYTES,
        })
    }

    pub fn for_encoded_record(bytes: &[u8]) -> Result<Self, LiveResourceArithmeticError> {
        let encoded_bytes = u64::try_from(bytes.len())
            .map_err(|_| LiveResourceArithmeticError::Overflow)?
            .checked_add(LIVE_RECORD_STORAGE_ALLOWANCE_BYTES)
            .ok_or(LiveResourceArithmeticError::Overflow)?;
        Ok(Self {
            records: 1,
            encoded_bytes,
        })
    }

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

    pub fn checked_mul(self, count: u64) -> Result<Self, LiveResourceArithmeticError> {
        Ok(Self {
            records: self
                .records
                .checked_mul(count)
                .ok_or(LiveResourceArithmeticError::Overflow)?,
            encoded_bytes: self
                .encoded_bytes
                .checked_mul(count)
                .ok_or(LiveResourceArithmeticError::Overflow)?,
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
    /// Record slots retained for terminal, late-outcome, and control facts.
    /// The byte capacity is measured from the versioned complete envelope,
    /// not estimated from decoded request/result text.
    pub const fn record_limit(self) -> u64 {
        match self {
            Self::ChannelControl => 64,
            Self::RequestChain | Self::EffectStart => 32,
            Self::FunctionOutput | Self::Continuation | Self::CallbackContinuation => 16,
            Self::ContextChunk => 12,
        }
    }

    pub fn base_budget(
        self,
    ) -> Result<LiveResourceCharge, crate::live_ledger::completion_budget::CompletionBudgetError>
    {
        crate::live_ledger::completion_budget::CompletionEnvelopeBudgetV1::for_obligation(self)
            .map(crate::live_ledger::completion_budget::CompletionEnvelopeBudgetV1::total)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveResourceArithmeticError {
    #[error("live resource charge overflows its integer representation")]
    Overflow,
    #[error("live resource charge subtraction exceeds the recorded charge")]
    Underflow,
}
