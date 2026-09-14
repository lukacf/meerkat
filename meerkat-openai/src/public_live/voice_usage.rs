//! Physical cumulative usage observations. The generated channel owner
//! reconciles duplicates, regressions, finality, and unconfirmed transport close.

use meerkat_core::live_execution::observation::{
    LiveUsageSnapshot, LiveVoiceDurationError, LiveVoiceDurationSeconds,
};
use oai_rt_rs::live::ServerEvent;

pub fn observe_voice_usage(
    event: &ServerEvent,
) -> Result<Option<LiveUsageSnapshot>, LiveVoiceDurationError> {
    match event {
        ServerEvent::UsageUpdated { usage, .. } => Ok(Some(LiveUsageSnapshot::Periodic {
            cumulative_seconds: LiveVoiceDurationSeconds::new(usage.seconds)?,
        })),
        ServerEvent::Closed { usage, .. } => Ok(Some(LiveUsageSnapshot::SessionClosed {
            cumulative_seconds: LiveVoiceDurationSeconds::new(usage.seconds)?,
        })),
        _ => Ok(None),
    }
}
