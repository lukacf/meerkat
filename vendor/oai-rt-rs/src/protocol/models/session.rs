use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

use super::{
    AudioConfig, AudioFormat, InputAudioTranscription, MaxTokens, Modality, Nullable,
    OutputModalities, PromptRef, Temperature, Tool, ToolChoice, TurnDetection, Voice,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    #[default]
    Realtime,
    Transcription,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TracingAuto {
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    pub workflow_name: Option<String>,
    pub group_id: Option<String>,
    /// Arbitrary tracing metadata (spec allows free-form JSON values).
    pub metadata: Option<super::Metadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Tracing {
    Auto(TracingAuto),
    Config(TracingConfig),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TruncationStrategy {
    Auto,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TruncationType {
    RetentionRatio,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenLimits {
    pub post_instructions: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetentionRatioTruncation {
    #[serde(rename = "type")]
    pub kind: TruncationType,
    pub retention_ratio: f32,
    pub token_limits: Option<TokenLimits>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Truncation {
    Strategy(TruncationStrategy),
    RetentionRatio(RetentionRatioTruncation),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(rename = "type")]
    pub kind: SessionKind,
    pub model: String,
    pub output_modalities: OutputModalities,
    pub modalities: Option<Vec<Modality>>,
    pub include: Option<Vec<String>>,
    pub prompt: Option<PromptRef>,
    pub truncation: Option<Truncation>,
    pub instructions: Option<String>,
    pub input_audio_format: Option<AudioFormat>,
    pub output_audio_format: Option<AudioFormat>,
    pub input_audio_transcription: Option<Nullable<InputAudioTranscription>>,
    pub turn_detection: Option<Nullable<TurnDetection>>,
    pub tools: Option<Vec<Tool>>,
    pub tool_choice: Option<ToolChoice>,
    pub temperature: Option<Temperature>,
    pub max_output_tokens: Option<MaxTokens>,
    pub audio: Option<AudioConfig>,
    pub tracing: Option<Tracing>,
    pub voice: Option<Voice>,
}

impl SessionConfig {
    #[must_use]
    pub fn new(
        kind: SessionKind,
        model: impl Into<String>,
        output_modalities: OutputModalities,
    ) -> Self {
        Self {
            kind,
            model: model.into(),
            output_modalities,
            modalities: None,
            include: None,
            prompt: None,
            truncation: None,
            instructions: None,
            input_audio_format: None,
            output_audio_format: None,
            input_audio_transcription: None,
            turn_detection: None,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_output_tokens: None,
            audio: None,
            tracing: None,
            voice: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionUpdateConfig {
    /// Partial updates only; GA forbids changing `model` or session `type`.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<SessionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_modalities: Option<OutputModalities>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modalities: Option<Vec<Modality>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<PromptRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_format: Option<AudioFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_audio_format: Option<AudioFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_audio_transcription: Option<Nullable<InputAudioTranscription>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_detection: Option<Nullable<TurnDetection>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<Temperature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<MaxTokens>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracing: Option<Tracing>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<Voice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub object: String,
    pub expires_at: u64,
    /// Flattened to match the API's session JSON shape.
    #[serde(flatten)]
    pub config: SessionConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SessionUpdate {
    /// Flattened to match the API's session.update JSON shape.
    #[serde(flatten)]
    pub config: SessionUpdateConfig,
}

impl Serialize for SessionUpdate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut map = serializer.serialize_map(None)?;
        let value = serde_json::to_value(&self.config).map_err(serde::ser::Error::custom)?;
        if let serde_json::Value::Object(obj) = value {
            if !obj.contains_key("type") {
                map.serialize_entry("type", "realtime")?;
            }
            for (k, v) in obj {
                map.serialize_entry(&k, &v)?;
            }
        }
        map.end()
    }
}
