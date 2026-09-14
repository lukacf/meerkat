use meerkat_core::live_execution::observation::{
    LiveUsageDispute, LiveUsageSnapshot, LiveVoiceDurationSeconds,
};
use sha2::{Digest, Sha256};

use super::dsl;
use crate::store::RuntimeStoreError;

pub(in crate::live_ledger) fn digest(
    kind: dsl::LiveVoiceUsageKind,
    seconds_bits: u64,
) -> Result<String, RuntimeStoreError> {
    let bytes = serde_json::to_vec(&(kind, seconds_bits))
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(in crate::live_ledger) fn input(
    channel: &str,
    sequence: u64,
    observation: &LiveUsageSnapshot,
) -> Result<dsl::LiveTranscriptInput, RuntimeStoreError> {
    use dsl::LiveVoiceUsageKind as Kind;
    let (kind, seconds) = match observation {
        LiveUsageSnapshot::Periodic { cumulative_seconds } => {
            (Kind::Periodic, cumulative_seconds.get())
        }
        LiveUsageSnapshot::SessionClosed { cumulative_seconds } => {
            (Kind::Final, cumulative_seconds.get())
        }
        // The provider's local cache is not the durable account. Closure
        // reconciles against the last value in the generated owner instead.
        LiveUsageSnapshot::CloseUnconfirmed { .. } => (Kind::ObservationClosed, 0.0),
        LiveUsageSnapshot::Disputed {
            last_valid_seconds: None,
            reason: LiveUsageDispute::InvalidDuration,
        } => (Kind::Invalid, 0.0),
        LiveUsageSnapshot::Disputed { .. } => {
            return Err(RuntimeStoreError::WriteFailed(
                "provider supplied a pre-adjudicated Live usage dispute".into(),
            ));
        }
    };
    // Nonnegative finite IEEE-754 encodings preserve numeric order exactly,
    // including subnormal values. Canonicalize only the two equal zeroes.
    let seconds_bits = if seconds == 0.0 { 0 } else { seconds.to_bits() };
    Ok(dsl::LiveTranscriptInput::ObserveVoiceUsage {
        channel: channel.into(),
        sequence,
        kind,
        seconds_bits,
        digest: digest(kind, seconds_bits)?,
        record_bytes: 1,
    })
}

pub(in crate::live_ledger) fn snapshot(
    state: &dsl::LiveTranscriptMachineState,
    channel: &str,
) -> Result<Option<LiveUsageSnapshot>, RuntimeStoreError> {
    let invalid = || RuntimeStoreError::ReadFailed("missing Live voice accounting state".into());
    let sequence = state
        .voice_usage_sequences
        .get(channel)
        .ok_or_else(invalid)?;
    if *sequence == 0 {
        return Ok(None);
    }
    let last = if state.voice_usage_observed.contains(channel) {
        Some(
            LiveVoiceDurationSeconds::new(f64::from_bits(
                *state.voice_seconds_bits.get(channel).ok_or_else(invalid)?,
            ))
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?,
        )
    } else {
        None
    };
    let dispute = match state
        .voice_usage_disputes
        .get(channel)
        .ok_or_else(invalid)?
    {
        dsl::LiveVoiceUsageDispute::None => None,
        dsl::LiveVoiceUsageDispute::Regression => Some(LiveUsageDispute::Regression),
        dsl::LiveVoiceUsageDispute::InvalidDuration => Some(LiveUsageDispute::InvalidDuration),
        dsl::LiveVoiceUsageDispute::ConflictingFinal => Some(LiveUsageDispute::ConflictingFinal),
    };
    Ok(Some(if let Some(reason) = dispute {
        LiveUsageSnapshot::Disputed {
            last_valid_seconds: last,
            reason,
        }
    } else if state.voice_final_observed.contains(channel) {
        LiveUsageSnapshot::SessionClosed {
            cumulative_seconds: last.ok_or_else(invalid)?,
        }
    } else if !state.voice_observation_open.contains(channel) {
        LiveUsageSnapshot::CloseUnconfirmed {
            last_observed_seconds: last,
        }
    } else {
        LiveUsageSnapshot::Periodic {
            cumulative_seconds: last.ok_or_else(invalid)?,
        }
    }))
}
