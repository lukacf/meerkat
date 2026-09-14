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

    pub const fn fits_within(self, limit: Self) -> bool {
        self.records <= limit.records && self.encoded_bytes <= limit.encoded_bytes
    }
}

/// Active-memory domains have separate ceilings from retained durable history.
/// In particular, a retired output still consumes durable quota but not an
/// unsettled-output slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveActiveResource {
    BackendCollections,
    Work,
    Outputs,
    Continuations,
    IncomingText,
    ProviderEvents,
    Diagnostics,
    ContextPlans,
}

impl LiveActiveResource {
    pub const ALL: [Self; 8] = [
        Self::BackendCollections,
        Self::Work,
        Self::Outputs,
        Self::Continuations,
        Self::IncomingText,
        Self::ProviderEvents,
        Self::Diagnostics,
        Self::ContextPlans,
    ];

    pub const fn limit(self) -> LiveResourceCharge {
        let (records, encoded_bytes) = match self {
            Self::BackendCollections => (32, 1024 * 1024),
            Self::Work | Self::Outputs => (128, 4 * 1024 * 1024),
            Self::Continuations => (1, 4 * 1024 * 1024),
            Self::IncomingText => (256, 1024 * 1024),
            Self::ProviderEvents => (256, 4 * 1024 * 1024),
            Self::Diagnostics => (64, 64 * 1024),
            Self::ContextPlans => (8, 64 * 1024),
        };
        LiveResourceCharge {
            records,
            encoded_bytes,
        }
    }
}

/// Bounded accounting image, not a reservation or retirement API. The
/// generated owner commits counters with the exact obligation disposition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveRetentionAccountingParts")]
pub struct LiveRetentionAccounting {
    used: LiveResourceCharge,
    reserved: LiveResourceCharge,
    quota: LiveResourceCharge,
    active: std::collections::BTreeMap<LiveActiveResource, LiveResourceCharge>,
    active_context_chunks: u16,
}

impl LiveRetentionAccounting {
    pub fn new(parts: LiveRetentionAccountingParts) -> Result<Self, LiveRetentionError> {
        if !parts.quota.fits_within(LIVE_LEDGER_MAX_CHARGE)
            || !parts
                .used
                .checked_add(parts.reserved)?
                .fits_within(parts.quota)
        {
            return Err(LiveRetentionError::DurableQuotaExceeded);
        }
        if parts.active.len() != LiveActiveResource::ALL.len()
            || LiveActiveResource::ALL
                .iter()
                .any(|kind| !parts.active.contains_key(kind))
        {
            return Err(LiveRetentionError::IncompleteActiveAccounting);
        }
        for (kind, charge) in &parts.active {
            if !charge.fits_within(kind.limit()) {
                return Err(LiveRetentionError::ActiveCapacityExceeded(*kind));
            }
        }
        let outputs = parts.active[&LiveActiveResource::Outputs];
        let continuations = parts.active[&LiveActiveResource::Continuations];
        if outputs
            .encoded_bytes
            .checked_add(continuations.encoded_bytes)
            .ok_or(LiveResourceArithmeticError::Overflow)?
            > LiveActiveResource::Outputs.limit().encoded_bytes
        {
            return Err(LiveRetentionError::SharedDeliveryCapacityExceeded);
        }
        if parts.active_context_chunks > 64
            || (parts.active[&LiveActiveResource::ContextPlans].records == 0
                && parts.active_context_chunks != 0)
        {
            return Err(LiveRetentionError::ContextChunkCapacityExceeded);
        }
        Ok(Self {
            used: parts.used,
            reserved: parts.reserved,
            quota: parts.quota,
            active: parts.active,
            active_context_chunks: parts.active_context_chunks,
        })
    }

    pub const fn used(&self) -> LiveResourceCharge {
        self.used
    }

    pub fn active(&self, kind: LiveActiveResource) -> LiveResourceCharge {
        self.active[&kind]
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRetentionAccountingParts {
    pub used: LiveResourceCharge,
    pub reserved: LiveResourceCharge,
    pub quota: LiveResourceCharge,
    pub active: std::collections::BTreeMap<LiveActiveResource, LiveResourceCharge>,
    pub active_context_chunks: u16,
}

impl TryFrom<LiveRetentionAccountingParts> for LiveRetentionAccounting {
    type Error = LiveRetentionError;

    fn try_from(parts: LiveRetentionAccountingParts) -> Result<Self, Self::Error> {
        Self::new(parts)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveRetentionError {
    #[error("Live durable used plus reserved charge exceeds the declared quota")]
    DurableQuotaExceeded,
    #[error("Live active accounting must cover every domain, including explicit zeros")]
    IncompleteActiveAccounting,
    #[error("Live active capacity exceeded for {0:?}")]
    ActiveCapacityExceeded(LiveActiveResource),
    #[error("Live output and continuation bytes exceed their shared capacity")]
    SharedDeliveryCapacityExceeded,
    #[error("Live context chunk count exceeds its bound or has no active plan")]
    ContextChunkCapacityExceeded,
    #[error(transparent)]
    Arithmetic(#[from] LiveResourceArithmeticError),
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
