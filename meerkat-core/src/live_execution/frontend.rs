//! Provider-neutral continuous frontend validation, before queue acceptance.
//! Channel/grant/generation authority is separate and must also be checked.

use crate::live_adapter::{LiveAdapterCommand, LiveInputChunk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveAudioIngress {
    PcmWebSocket { sample_rate_hz: u32, channels: u16 },
    WebRtcMediaTracks,
    SidebandOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuousLiveFrontendPolicy {
    audio_ingress: LiveAudioIngress,
}

impl ContinuousLiveFrontendPolicy {
    pub const fn audio_ingress(&self) -> LiveAudioIngress {
        self.audio_ingress
    }
    pub fn new(audio_ingress: LiveAudioIngress) -> Result<Self, ContinuousLiveInputError> {
        if matches!(
            audio_ingress,
            LiveAudioIngress::PcmWebSocket {
                sample_rate_hz: 0,
                ..
            } | LiveAudioIngress::PcmWebSocket { channels: 0, .. }
        ) {
            return Err(ContinuousLiveInputError::InvalidAudioFormat);
        }
        Ok(Self { audio_ingress })
    }

    /// This is the controller command boundary, not the raw browser data
    /// channel (whose allowed command list is empty).
    pub fn validate(&self, command: &LiveAdapterCommand) -> Result<(), ContinuousLiveInputError> {
        match command {
            LiveAdapterCommand::Close => Ok(()),
            LiveAdapterCommand::SendInput { chunk } => match chunk {
                LiveInputChunk::Audio {
                    data,
                    sample_rate_hz,
                    channels,
                } => {
                    let LiveAudioIngress::PcmWebSocket {
                        sample_rate_hz: expected_rate,
                        channels: expected_channels,
                    } = self.audio_ingress
                    else {
                        return Err(ContinuousLiveInputError::AudioUsesAnotherTransport);
                    };
                    if *sample_rate_hz != expected_rate || *channels != expected_channels {
                        return Err(ContinuousLiveInputError::InvalidAudioFormat);
                    }
                    let frame_bytes = usize::from(expected_channels) * 2;
                    let two_seconds = u64::from(expected_rate) * u64::from(expected_channels) * 4;
                    if data.is_empty()
                        || !data.len().is_multiple_of(frame_bytes)
                        || data.len() as u64 > two_seconds.min(2 * 1024 * 1024)
                    {
                        return Err(ContinuousLiveInputError::InvalidAudioFrame);
                    }
                    Ok(())
                }
                LiveInputChunk::Text { .. } => Err(ContinuousLiveInputError::UnsupportedInputKind),
                LiveInputChunk::Image { .. } | LiveInputChunk::VideoFrame { .. } => {
                    Err(ContinuousLiveInputError::UnsupportedFrontendModality)
                }
            },
            LiveAdapterCommand::CommitInput { .. }
            | LiveAdapterCommand::Interrupt
            | LiveAdapterCommand::TruncateAssistantOutput { .. }
            | LiveAdapterCommand::CompleteAssistantPlayback { .. } => {
                Err(ContinuousLiveInputError::UnsupportedCapability)
            }
            LiveAdapterCommand::Open { .. }
            | LiveAdapterCommand::Refresh { .. }
            | LiveAdapterCommand::SubmitToolResult { .. }
            | LiveAdapterCommand::SubmitToolError { .. } => {
                Err(ContinuousLiveInputError::RequiresOwnerControl)
            }
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
    thiserror::Error,
)]
#[serde(rename_all = "snake_case")]
pub enum ContinuousLiveInputError {
    #[error("continuous Live does not accept this frontend input kind")]
    UnsupportedInputKind,
    #[error("continuous Live does not accept frontend image/video input")]
    UnsupportedFrontendModality,
    #[error(
        "continuous Live does not support turn commit, barge-in, truncation or playback completion"
    )]
    UnsupportedCapability,
    #[error("this operation requires the profile/request owner, not a frontend command")]
    RequiresOwnerControl,
    #[error(
        "audio uses negotiated WebRTC tracks or the primary connection, not this command route"
    )]
    AudioUsesAnotherTransport,
    #[error("audio metadata does not match the bound positive PCM format")]
    InvalidAudioFormat,
    #[error(
        "audio must be whole PCM16 frames within the channel-normalized duration and byte bounds"
    )]
    InvalidAudioFrame,
}
