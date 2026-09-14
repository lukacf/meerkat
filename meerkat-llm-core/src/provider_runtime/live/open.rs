//! Static open-request validation. These intents remain content until the
//! owning host resolves the selected profile and issues an exact activation.

use std::num::NonZeroUsize;

use meerkat_contracts::{RealtimeTurningMode, WireLiveExecutionIdentityOverrideV1};
use meerkat_core::live_execution::profile::LiveProfileId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOpenIntent<'a> {
    Realtime {
        execution_identity: Option<&'a WireLiveExecutionIdentityOverrideV1>,
        turning_mode: RealtimeTurningMode,
        seed_max_chars: Option<NonZeroUsize>,
    },
    Continuous {
        profile_id: &'a LiveProfileId,
    },
}

impl<'a> LiveOpenIntent<'a> {
    /// `None` means omitted, never an explicitly-null profile selector. Wire
    /// decoding must reject null before reaching this typed boundary.
    pub fn select(
        profile_id: Option<&'a LiveProfileId>,
        execution_identity: Option<&'a WireLiveExecutionIdentityOverrideV1>,
        turning_mode: Option<RealtimeTurningMode>,
        seed_max_chars: Option<usize>,
    ) -> Result<Self, LiveOpenIntentError> {
        if let Some(profile_id) = profile_id {
            if execution_identity.is_some() {
                return Err(LiveOpenIntentError::MutuallyExclusiveSelection);
            }
            if turning_mode.is_some_and(|mode| mode != RealtimeTurningMode::Continuous) {
                return Err(LiveOpenIntentError::UnsupportedTurningMode);
            }
            if seed_max_chars.is_some() {
                return Err(LiveOpenIntentError::UnsupportedSeedWindowForContinuous);
            }
            return Ok(Self::Continuous { profile_id });
        }
        let turning_mode = turning_mode.unwrap_or(RealtimeTurningMode::ProviderManaged);
        if turning_mode == RealtimeTurningMode::Continuous {
            return Err(LiveOpenIntentError::UnsupportedTurningMode);
        }
        let seed_max_chars = seed_max_chars
            .map(|limit| NonZeroUsize::new(limit).ok_or(LiveOpenIntentError::ZeroSeedWindow))
            .transpose()?;
        Ok(Self::Realtime {
            execution_identity,
            turning_mode,
            seed_max_chars,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveOpenIntentError {
    #[error("profile_id and execution_identity select different Live paths and cannot be combined")]
    MutuallyExclusiveSelection,
    #[error("the selected Live path does not support this turning mode")]
    UnsupportedTurningMode,
    #[error("continuous Live uses its profile disclosure policy, not a legacy seed window")]
    UnsupportedSeedWindowForContinuous,
    #[error("the realtime seed window must be positive")]
    ZeroSeedWindow,
}
