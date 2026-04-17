use serde::{Deserialize, Serialize};

use super::{Eagerness, Nullable, Voice};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AudioFormat {
    #[serde(rename = "audio/pcm")]
    Pcm {
        #[serde(default = "default_pcm_rate")]
        rate: u32,
    },
    #[serde(rename = "audio/pcmu")]
    Pcmu,
    #[serde(rename = "audio/pcma")]
    Pcma,
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pcm { .. } => write!(f, "audio/pcm"),
            Self::Pcmu => write!(f, "audio/pcmu"),
            Self::Pcma => write!(f, "audio/pcma"),
        }
    }
}

const PCM_24KHZ_RATE: u32 = 24_000;

const fn default_pcm_rate() -> u32 {
    PCM_24KHZ_RATE
}

impl AudioFormat {
    #[must_use]
    pub const fn pcm_24khz() -> Self {
        Self::Pcm {
            rate: PCM_24KHZ_RATE,
        }
    }

    /// # Errors
    /// Returns an error if a PCM format is configured with a non-24kHz rate.
    #[allow(clippy::result_large_err)]
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        match self {
            Self::Pcm { rate } if *rate != PCM_24KHZ_RATE => {
                Err(crate::error::Error::InvalidClientEvent(format!(
                    "audio/pcm rate must be {PCM_24KHZ_RATE}, got {rate}"
                )))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AudioConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<InputAudioConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<OutputAudioConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InputAudioConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<AudioFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_detection: Option<Nullable<TurnDetection>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcription: Option<Nullable<InputAudioTranscription>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noise_reduction: Option<Nullable<NoiseReduction>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NoiseReduction {
    #[serde(rename = "type")]
    pub kind: NoiseReductionType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NoiseReductionType {
    #[default]
    NearField,
    FarField,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OutputAudioConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<AudioFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<Voice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InputAudioTranscription {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TurnDetection {
    ServerVad {
        #[serde(skip_serializing_if = "Option::is_none")]
        threshold: Option<f32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefix_padding_ms: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        silence_duration_ms: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        idle_timeout_ms: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        create_response: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupt_response: Option<bool>,
    },
    SemanticVad {
        #[serde(skip_serializing_if = "Option::is_none")]
        eagerness: Option<Eagerness>,
        #[serde(skip_serializing_if = "Option::is_none")]
        create_response: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupt_response: Option<bool>,
    },
}
