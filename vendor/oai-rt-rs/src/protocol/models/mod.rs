pub mod audio;
pub mod common;
pub mod items;
pub mod response;
pub mod session;
pub mod tools;
pub mod usage;

pub use audio::{
    AudioConfig, AudioFormat, InputAudioConfig, InputAudioTranscription, NoiseReduction,
    NoiseReductionType, OutputAudioConfig, TurnDetection,
};
pub use common::{
    ArbitraryJson, DEFAULT_MODEL, Eagerness, Infinite, ItemStatus, JsonSchema, MaxTokens, Metadata,
    Modality, Nullable, OutputModalities, PromptRef, Role, Temperature, TemperatureError, Voice,
};
pub use items::{AudioPartFormat, ContentPart, Item};
pub use response::{
    ConversationMode, InputItem, Response, ResponseConfig, ResponseStatus, ResponseStatusDetails,
};
pub use session::{
    RetentionRatioTruncation, Session, SessionConfig, SessionKind, SessionUpdate,
    SessionUpdateConfig, TokenLimits, Tracing, TracingAuto, TracingConfig, Truncation,
    TruncationStrategy, TruncationType,
};
pub use tools::{
    ApprovalFilter, ApprovalMode, McpError, McpToolConfig, McpToolInfo, RequireApproval, Tool,
    ToolChoice, ToolChoiceMode,
};
pub use usage::{CachedTokenDetails, InputTokenDetails, OutputTokenDetails, Usage};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_tokens_infinite() {
        let inf = MaxTokens::Infinite(Infinite::Inf);
        let serialized = serde_json::to_string(&inf).unwrap();
        assert_eq!(serialized, "\"inf\"");
        let deserialized: MaxTokens = serde_json::from_str(&serialized).unwrap();
        assert!(matches!(deserialized, MaxTokens::Infinite(Infinite::Inf)));
    }
}
