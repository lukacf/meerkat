//! Typed substrate for assistant image generation and scoped model routing.
//!
//! This module contains representation types only. Runtime policy, provider
//! calls, blob I/O, and machine transitions live outside `meerkat-core`.

use crate::approval::ApprovalId;
use crate::blob::BlobRef;
use crate::lifecycle::run_primitive::ModelId;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::num::NonZeroU32;
use uuid::Uuid;

pub const DEFAULT_PROMPT_TEXT_MAX_CHARS: usize = 32_000;
pub const DEFAULT_SWITCH_TURN_REASON_MAX_CHARS: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImageGenerationValidationError {
    #[error("{field} must not be empty")]
    EmptyText { field: &'static str },
    #[error("{field} exceeds maximum length of {max_chars} characters")]
    TextTooLong {
        field: &'static str,
        max_chars: usize,
    },
    #[error("edit image generation intent requires at least one source image")]
    MissingEditSourceImages,
}

fn validate_text(
    content: &str,
    field: &'static str,
    max_chars: usize,
) -> Result<(), ImageGenerationValidationError> {
    if content.trim().is_empty() {
        return Err(ImageGenerationValidationError::EmptyText { field });
    }
    if content.chars().count() > max_chars {
        return Err(ImageGenerationValidationError::TextTooLong { field, max_chars });
    }
    Ok(())
}

macro_rules! uuid_id {
    ($name:ident) => {
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

        impl $name {
            pub fn new(id: Uuid) -> Self {
                Self(id)
            }
        }
    };
}

uuid_id!(AssistantImageId);
uuid_id!(ImageOperationId);
uuid_id!(SwitchTurnRequestId);
uuid_id!(ScopedModelOverrideId);
uuid_id!(ProjectionSnapshotId);
uuid_id!(MessageId);
uuid_id!(BlockId);

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ToolCallId(pub String);

impl ToolCallId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(pub String);

impl ProviderId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderImageHandle(pub String);

impl ProviderImageHandle {
    pub fn new(handle: impl Into<String>) -> Self {
        Self(handle.into())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImageContinuityRef(pub String);

impl ImageContinuityRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TextArtifactRef(pub String);

impl TextArtifactRef {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaType(pub String);

impl MediaType {
    pub fn new(media_type: impl Into<String>) -> Self {
        Self(media_type.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TopologyEpoch(pub u64);

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptText {
    pub content: String,
}

impl PromptText {
    pub fn new(content: impl Into<String>) -> Result<Self, ImageGenerationValidationError> {
        Self::with_max_chars(content, DEFAULT_PROMPT_TEXT_MAX_CHARS)
    }

    pub fn with_max_chars(
        content: impl Into<String>,
        max_chars: usize,
    ) -> Result<Self, ImageGenerationValidationError> {
        let content = content.into();
        validate_text(&content, "prompt_text", max_chars)?;
        Ok(Self { content })
    }
}

impl<'de> Deserialize<'de> for PromptText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawPromptText {
            content: String,
        }

        let raw = RawPromptText::deserialize(deserializer)?;
        PromptText::new(raw.content).map_err(serde::de::Error::custom)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PromptSource {
    UserProvided {
        message_id: MessageId,
    },
    ModelDistilled {
        tool_call_id: ToolCallId,
    },
    Hybrid {
        user_message_ids: Vec<MessageId>,
        tool_call_id: ToolCallId,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageSourceRef {
    Blob {
        blob_ref: BlobRef,
    },
    TranscriptBlock {
        message_id: MessageId,
        block_id: BlockId,
    },
    AssistantImage {
        image_id: AssistantImageId,
    },
    ProviderNative {
        provider: ProviderId,
        handle: ProviderImageHandle,
        continuity: ImageContinuityDisposition,
        fallback_blob_ref: BlobRef,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum ImageContinuityDisposition {
    NotProvided,
    UnsupportedBySourceProvider,
    Available { continuity_ref: ImageContinuityRef },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum ImageGenerationIntent {
    Generate {
        prompt: PromptText,
        prompt_source: PromptSource,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reference_images: Vec<ImageSourceRef>,
    },
    Edit {
        instruction: PromptText,
        instruction_source: PromptSource,
        source_images: Vec<ImageSourceRef>,
    },
}

impl ImageGenerationIntent {
    pub fn edit(
        instruction: PromptText,
        instruction_source: PromptSource,
        source_images: Vec<ImageSourceRef>,
    ) -> Result<Self, ImageGenerationValidationError> {
        if source_images.is_empty() {
            return Err(ImageGenerationValidationError::MissingEditSourceImages);
        }
        Ok(Self::Edit {
            instruction,
            instruction_source,
            source_images,
        })
    }

    pub fn validate(&self) -> Result<(), ImageGenerationValidationError> {
        if matches!(self, Self::Edit { source_images, .. } if source_images.is_empty()) {
            return Err(ImageGenerationValidationError::MissingEditSourceImages);
        }
        Ok(())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum ImageGenerationTargetPreference {
    Auto,
    ProviderDefault {
        provider: ProviderId,
    },
    Model {
        provider: ProviderId,
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        model: ModelId,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "size", rename_all = "snake_case")]
pub enum ImageSizePreference {
    Auto,
    Square1024,
    Portrait1024x1536,
    Landscape1536x1024,
    Custom {
        width: NonZeroU32,
        height: NonZeroU32,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageQualityPreference {
    Auto,
    Low,
    Medium,
    High,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormatPreference {
    Auto,
    Png,
    Jpeg,
    Webp,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GenerateImageRequest {
    pub intent: ImageGenerationIntent,
    pub target: ImageGenerationTargetPreference,
    pub size: ImageSizePreference,
    pub quality: ImageQualityPreference,
    pub format: ImageFormatPreference,
    pub count: NonZeroU32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<Value>,
}

impl GenerateImageRequest {
    pub fn new(
        intent: ImageGenerationIntent,
        target: ImageGenerationTargetPreference,
        size: ImageSizePreference,
        quality: ImageQualityPreference,
        format: ImageFormatPreference,
        count: NonZeroU32,
    ) -> Result<Self, ImageGenerationValidationError> {
        Self::with_provider_params(intent, target, size, quality, format, count, None)
    }

    pub fn with_provider_params(
        intent: ImageGenerationIntent,
        target: ImageGenerationTargetPreference,
        size: ImageSizePreference,
        quality: ImageQualityPreference,
        format: ImageFormatPreference,
        count: NonZeroU32,
        provider_params: Option<Value>,
    ) -> Result<Self, ImageGenerationValidationError> {
        intent.validate()?;
        Ok(Self {
            intent,
            target,
            size,
            quality,
            format,
            count,
            provider_params,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationResolvedPlan {
    pub provider_model: ModelId,
    pub machine_routing_model: ModelId,
    pub machine_routing_realtime_capable: bool,
    pub execution_plan: GenerateImageExecutionPlan,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projected_messages: Vec<crate::Message>,
}

impl<'de> Deserialize<'de> for GenerateImageRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawGenerateImageRequest {
            intent: ImageGenerationIntent,
            target: ImageGenerationTargetPreference,
            size: ImageSizePreference,
            quality: ImageQualityPreference,
            format: ImageFormatPreference,
            count: NonZeroU32,
            #[serde(default)]
            provider_params: Option<Value>,
        }

        let raw = RawGenerateImageRequest::deserialize(deserializer)?;
        GenerateImageRequest::with_provider_params(
            raw.intent,
            raw.target,
            raw.size,
            raw.quality,
            raw.format,
            raw.count,
            raw.provider_params,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageContinuityTokenSupport {
    Unsupported,
    SameProviderOnly,
    CrossProvider,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageGenerationTargetCapabilities {
    pub hosted_image_generation_tool: bool,
    pub native_image_output: bool,
    pub custom_tools: bool,
    pub image_search_grounding: bool,
    pub image_continuity_tokens: ImageContinuityTokenSupport,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerateImageExecutionPlan {
    pub provider: ProviderId,
    pub backend: ImageGenerationBackendKind,
    pub max_count: NonZeroU32,
    pub capabilities: ImageGenerationTargetCapabilities,
    pub requires_scoped_override: bool,
    #[serde(default)]
    pub provider_plan: Value,
}

impl GenerateImageExecutionPlan {
    pub fn requires_scoped_override(&self) -> bool {
        self.requires_scoped_override
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageGenerationBackendKind {
    HostedTool,
    ProviderApi,
    NativeModel,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationProviderResolution {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub provider_call_model: ModelId,
    pub execution_plan: GenerateImageExecutionPlan,
}

pub trait ImageGenerationProviderProfile: Send + Sync {
    fn canonical_provider(&self) -> &'static str;

    fn provider_aliases(&self) -> &'static [&'static str] {
        &[]
    }

    fn matches_provider_id(&self, provider: &str) -> bool {
        let provider = provider.to_ascii_lowercase();
        provider == self.canonical_provider()
            || self.provider_aliases().contains(&provider.as_str())
    }

    fn default_model_for_session(
        &self,
        effective_provider: Option<crate::Provider>,
        effective_model: &ModelId,
    ) -> ModelId;

    fn owns_model(&self, model: &str) -> bool;

    fn image_generation_documentation(&self) -> Option<&'static str> {
        None
    }

    fn resolve_execution_plan(
        &self,
        operation_id: ImageOperationId,
        requested_model: &ModelId,
        request: &GenerateImageRequest,
        capabilities: ImageGenerationTargetCapabilities,
        max_count: NonZeroU32,
    ) -> Result<ImageGenerationProviderResolution, ImageOperationDenialReason>;
}

pub trait ImageGenerationPlanner: Send + Sync {
    fn resolve_image_generation_plan(
        &self,
        status: &SessionModelRoutingStatus,
        operation_id: ImageOperationId,
        request: &GenerateImageRequest,
    ) -> Result<ImageGenerationResolvedPlan, ImageOperationDenialReason>;

    fn infer_provider_for_model(&self, model: &str) -> Option<ProviderId>;

    fn provider_documentation(&self) -> Vec<String> {
        Vec::new()
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantImageRef {
    pub image_id: AssistantImageId,
    pub blob_ref: BlobRef,
    pub media_type: MediaType,
    pub width: u32,
    pub height: u32,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum ProviderTextDisposition {
    NotEmitted,
    UnsupportedByBackend,
    EmittedButNotStored,
    Captured { text_artifact_ref: TextArtifactRef },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum RevisedPromptDisposition {
    NotRequested,
    UnsupportedByBackend,
    Unchanged,
    Revised {
        text: PromptText,
        source: RevisedPromptSource,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisedPromptSource {
    Provider,
    MeerkatProjection,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "warning", rename_all = "snake_case")]
pub enum ImageGenerationWarning {
    ContinuityDegraded,
    UnsupportedArtifactDropped,
    ProviderReturnedFewerImages {
        requested: NonZeroU32,
        returned: NonZeroU32,
    },
    ProviderExecutionFailed {
        message: String,
    },
    BlobCommitFailed {
        message: String,
    },
    ProviderTextCaptureFailed {
        message: String,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderImageMetadata {
    NotEmitted,
    OpenAi(OpenAiImageMetadata),
    Gemini(GeminiImageMetadata),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiImageMetadata {
    pub target_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_generation_call_id: Option<String>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeminiImageMetadata {
    pub target_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuity_ref: Option<ImageContinuityRef>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageGenerationToolResult {
    pub operation_id: ImageOperationId,
    pub terminal: ImageOperationTerminalClass,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<AssistantImageRef>,
    pub provider_text: ProviderTextDisposition,
    pub revised_prompt: RevisedPromptDisposition,
    pub native_metadata: ProviderImageMetadata,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ImageGenerationWarning>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ImageOperationPhase {
    Requested,
    Validating,
    AwaitingApproval {
        approval_id: ApprovalId,
    },
    PlanResolved,
    ProjectionSnapshotted,
    ScopedOverrideActive,
    ProviderCallInFlight,
    ProviderResultCaptured,
    BlobCommitPending,
    ResultCommitted,
    RestoringScopedOverride,
    Terminal {
        terminal: ImageOperationTerminalClass,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "terminal", rename_all = "snake_case")]
pub enum ImageOperationTerminalClass {
    Generated,
    EmptyResult {
        provider_text: ProviderTextDisposition,
    },
    Denied {
        reason: ImageOperationDenialReason,
    },
    RefusedByProvider,
    SafetyFiltered,
    Failed,
    Cancelled,
    Timeout,
    ScopedRestoreFailed {
        trigger: PostActivationImageTerminal,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ImageOperationDenialReason {
    UnsupportedTarget,
    UnsupportedCount,
    CapabilityPolicy,
    CostPolicy,
    SafetyPolicy,
    ApprovalRequiredButUnavailable,
    DeniedDuringApproval {
        approvable: ImageOperationApprovalReason,
    },
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "terminal", rename_all = "snake_case")]
pub enum PostActivationImageTerminal {
    Generated,
    EmptyResult,
    Denied {
        reason: PostActivationImageDenialReason,
    },
    RefusedByProvider,
    SafetyFiltered,
    Failed,
    Cancelled,
    Timeout,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PostActivationImageDenialReason {
    CostPolicy,
    SafetyPolicy,
    DeniedDuringApproval {
        approvable: ImageOperationApprovalReason,
    },
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchTurnIntent {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub target_model: ModelId,
    pub duration: SwitchTurnDuration,
    pub origin: SwitchTurnOrigin,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "duration_type", rename_all = "snake_case")]
pub enum SwitchTurnDuration {
    Finite { duration: FiniteScopedTurnDuration },
    UntilChanged,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "duration", rename_all = "snake_case")]
pub enum FiniteScopedTurnDuration {
    OneTurn,
    Turns { turns: NonZeroU32 },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SwitchTurnOrigin {
    User {
        reason: SwitchTurnReasonTextDisposition,
    },
    Model {
        reason: SwitchTurnReasonTextDisposition,
    },
    SystemPolicy {
        reason: SwitchTurnPolicyReason,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum SwitchTurnReasonTextDisposition {
    NotProvided,
    Provided { reason: SwitchTurnReasonText },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SwitchTurnReasonText {
    pub content: String,
}

impl SwitchTurnReasonText {
    pub fn new(content: impl Into<String>) -> Result<Self, ImageGenerationValidationError> {
        let content = content.into();
        validate_text(
            &content,
            "switch_turn_reason_text",
            DEFAULT_SWITCH_TURN_REASON_MAX_CHARS,
        )?;
        Ok(Self { content })
    }
}

impl<'de> Deserialize<'de> for SwitchTurnReasonText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSwitchTurnReasonText {
            content: String,
        }

        let raw = RawSwitchTurnReasonText::deserialize(deserializer)?;
        SwitchTurnReasonText::new(raw.content).map_err(serde::de::Error::custom)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchTurnPolicyReason {
    BudgetDowngrade,
    SafetyHandoff,
    ReleaseTemporaryHold,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SwitchTurnControlResult {
    Applied {
        request_id: SwitchTurnRequestId,
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        target_model: ModelId,
        duration: SwitchTurnDuration,
    },
    AwaitingApproval {
        approval_id: ApprovalId,
        #[cfg_attr(feature = "schema", schemars(with = "String"))]
        target_model: ModelId,
        duration: SwitchTurnDuration,
        reason: SwitchTurnApprovalReason,
    },
    Denied {
        request_id: SwitchTurnRequestId,
        reason: SwitchTurnDenialReason,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SwitchTurnDenialReason {
    UnsupportedModel,
    CapabilityPolicy,
    CostPolicy,
    SafetyPolicy,
    ApprovalRequiredButUnavailable,
    DeniedDuringApproval {
        approvable: SwitchTurnApprovalReason,
    },
    ScopedOverrideConflict,
    RealtimeTransportConflict,
    ProjectionUnsupported,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchTurnApprovalReason {
    CrossProvider,
    CostExceedsThreshold,
    SafetyHold,
    UntilChangedFromModelOrigin,
    RealtimeDetachRequired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageOperationApprovalReason {
    CrossProvider,
    CostExceedsThreshold,
    SafetyHold,
    RealtimeDetachRequired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRoutingApprovalTerminalClass {
    Approved,
    DeniedByUser,
    Expired,
    Interrupted,
    SessionArchived,
    SurfaceDetached,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ModelRoutingApprovalPhase {
    Pending,
    PresentedToUser,
    Terminal {
        terminal: ModelRoutingApprovalTerminalClass,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "parent", rename_all = "snake_case")]
pub enum ModelRoutingApprovalRequest {
    SwitchTurn {
        request_id: SwitchTurnRequestId,
        reason: SwitchTurnApprovalReason,
    },
    ImageOperation {
        operation_id: ImageOperationId,
        reason: ImageOperationApprovalReason,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum SwitchTurnPhase {
    Requested,
    Validating,
    PendingForBoundary,
    AwaitingApproval { approval_id: ApprovalId },
    ApplyingFiniteOverride,
    ApplyingPersistentReconfigure,
    ActiveFiniteOverride,
    RestoringFiniteOverride,
    Terminal { terminal: SwitchTurnTerminalClass },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "terminal", rename_all = "snake_case")]
pub enum SwitchTurnTerminalClass {
    Denied { reason: SwitchTurnDenialReason },
    ConsumedAndRestored,
    InterruptedAndRestored,
    PersistentReconfigureApplied,
    PersistentReconfigureFailed,
    RestoreFailed { trigger: SwitchTurnRestoreTrigger },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchTurnRestoreTrigger {
    Consumed,
    Interrupted,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedModelOverride {
    pub id: ScopedModelOverrideId,
    pub kind: ScopedModelOverrideKind,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub previous_effective_model: ModelId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub baseline_model_snapshot: ModelId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub target_model: ModelId,
    pub topology_epoch: TopologyEpoch,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopedModelOverrideKind {
    ImageOperation {
        operation_id: ImageOperationId,
    },
    FiniteSwitchTurn {
        request_id: SwitchTurnRequestId,
        duration: FiniteScopedTurnDuration,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopedModelOverrideSummary {
    pub id: ScopedModelOverrideId,
    pub kind: ScopedModelOverrideKind,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub target_model: ModelId,
    pub topology_epoch: TopologyEpoch,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchTurnRequestSummary {
    pub request_id: SwitchTurnRequestId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub target_model: ModelId,
    pub duration: SwitchTurnDuration,
    pub phase: SwitchTurnPhase,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionModelRoutingStatus {
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub baseline_model: ModelId,
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub effective_model: ModelId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_turn_override: Option<ScopedModelOverrideSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_operation_override: Option<ScopedModelOverrideSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_switch_turn: Option<SwitchTurnRequestSummary>,
}

impl SessionModelRoutingStatus {
    pub fn new(
        baseline_model: ModelId,
        active_turn_override: Option<ScopedModelOverrideSummary>,
        active_operation_override: Option<ScopedModelOverrideSummary>,
        pending_switch_turn: Option<SwitchTurnRequestSummary>,
    ) -> Self {
        let effective_model = active_operation_override
            .as_ref()
            .or(active_turn_override.as_ref())
            .map(|summary| summary.target_model.clone())
            .unwrap_or_else(|| baseline_model.clone());

        Self {
            baseline_model,
            effective_model,
            active_turn_override,
            active_operation_override,
            pending_switch_turn,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::redundant_clone)]
mod tests {
    use super::*;
    use crate::blob::{BlobId, BlobRef};
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn roundtrip<T>(value: T)
    where
        T: Serialize + DeserializeOwned + PartialEq + Debug,
    {
        let encoded = serde_json::to_string(&value).unwrap();
        let decoded: T = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, value);
    }

    fn source_image() -> ImageSourceRef {
        ImageSourceRef::Blob {
            blob_ref: BlobRef {
                blob_id: BlobId::new("blob-1"),
                media_type: "image/png".to_string(),
            },
        }
    }

    fn prompt_source() -> PromptSource {
        PromptSource::ModelDistilled {
            tool_call_id: ToolCallId::new("tool-call-1"),
        }
    }

    #[test]
    fn image_generation_request_serde_roundtrip() {
        let request = GenerateImageRequest::new(
            ImageGenerationIntent::Generate {
                prompt: PromptText::new("paint a small red bridge").unwrap(),
                prompt_source: prompt_source(),
                reference_images: vec![source_image()],
            },
            ImageGenerationTargetPreference::ProviderDefault {
                provider: ProviderId::new("openai"),
            },
            ImageSizePreference::Square1024,
            ImageQualityPreference::High,
            ImageFormatPreference::Png,
            NonZeroU32::new(2).unwrap(),
        )
        .unwrap();

        let json = serde_json::to_string(&request).unwrap();
        let parsed: GenerateImageRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, request);
    }

    #[test]
    fn nonzero_count_validation_rejects_zero() {
        let json = serde_json::json!({
            "intent": {
                "intent": "generate",
                "prompt": { "content": "draw a circle" },
                "prompt_source": {
                    "source": "model_distilled",
                    "tool_call_id": "tool-call-1"
                }
            },
            "target": { "target": "auto" },
            "size": { "size": "auto" },
            "quality": "auto",
            "format": "png",
            "count": 0
        });

        assert!(serde_json::from_value::<GenerateImageRequest>(json).is_err());
    }

    #[test]
    fn edit_request_deserialization_rejects_empty_source_images() {
        let err = ImageGenerationIntent::edit(
            PromptText::new("make it brighter").unwrap(),
            prompt_source(),
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err, ImageGenerationValidationError::MissingEditSourceImages);

        let json = serde_json::json!({
            "intent": {
                "intent": "edit",
                "instruction": { "content": "make it brighter" },
                "instruction_source": {
                    "source": "model_distilled",
                    "tool_call_id": "tool-call-1"
                },
                "source_images": []
            },
            "target": { "target": "auto" },
            "size": { "size": "auto" },
            "quality": "auto",
            "format": "png",
            "count": 1
        });
        assert!(serde_json::from_value::<GenerateImageRequest>(json).is_err());
    }

    #[test]
    fn execution_plan_and_result_variants_roundtrip() {
        let caps = ImageGenerationTargetCapabilities {
            hosted_image_generation_tool: true,
            native_image_output: false,
            custom_tools: true,
            image_search_grounding: false,
            image_continuity_tokens: ImageContinuityTokenSupport::SameProviderOnly,
        };
        let plans = vec![
            GenerateImageExecutionPlan {
                provider: ProviderId::new("openai"),
                backend: ImageGenerationBackendKind::HostedTool,
                max_count: NonZeroU32::new(4).unwrap(),
                capabilities: caps.clone(),
                requires_scoped_override: false,
                provider_plan: serde_json::json!({"tool_name": "image_generation"}),
            },
            GenerateImageExecutionPlan {
                provider: ProviderId::new("openai"),
                backend: ImageGenerationBackendKind::ProviderApi,
                max_count: NonZeroU32::new(1).unwrap(),
                capabilities: caps.clone(),
                requires_scoped_override: false,
                provider_plan: serde_json::json!({"endpoint": "generations"}),
            },
            GenerateImageExecutionPlan {
                provider: ProviderId::new("openai"),
                backend: ImageGenerationBackendKind::ProviderApi,
                max_count: NonZeroU32::new(1).unwrap(),
                capabilities: caps.clone(),
                requires_scoped_override: false,
                provider_plan: serde_json::json!({"endpoint": "edits"}),
            },
            GenerateImageExecutionPlan {
                provider: ProviderId::new("gemini"),
                backend: ImageGenerationBackendKind::NativeModel,
                max_count: NonZeroU32::new(1).unwrap(),
                capabilities: ImageGenerationTargetCapabilities {
                    hosted_image_generation_tool: false,
                    native_image_output: true,
                    custom_tools: false,
                    image_search_grounding: false,
                    image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
                },
                requires_scoped_override: true,
                provider_plan: serde_json::json!({
                    "projection_snapshot_id": ProjectionSnapshotId::new(uuid(50))
                }),
            },
        ];

        for plan in plans {
            let encoded = serde_json::to_string(&plan).unwrap();
            let decoded: GenerateImageExecutionPlan = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, plan);
        }

        let result = ImageGenerationToolResult {
            operation_id: ImageOperationId::new(uuid(51)),
            terminal: ImageOperationTerminalClass::Generated,
            images: vec![AssistantImageRef {
                image_id: AssistantImageId::new(uuid(52)),
                blob_ref: BlobRef {
                    blob_id: BlobId::new("blob-52"),
                    media_type: "image/png".into(),
                },
                media_type: MediaType::new("image/png"),
                width: 1024,
                height: 1024,
            }],
            provider_text: ProviderTextDisposition::Captured {
                text_artifact_ref: TextArtifactRef::new("text-artifact-1"),
            },
            revised_prompt: RevisedPromptDisposition::Unchanged,
            native_metadata: ProviderImageMetadata::Gemini(GeminiImageMetadata {
                target_model: "provider-native-image-model".into(),
                response_id: Some("gemini-response".into()),
                continuity_ref: Some(ImageContinuityRef::new("continuity-1")),
            }),
            warnings: vec![ImageGenerationWarning::ContinuityDegraded],
        };
        let encoded = serde_json::to_string(&result).unwrap();
        let decoded: ImageGenerationToolResult = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, result);
    }

    #[test]
    fn effective_model_precedence_is_operation_then_turn_then_baseline() {
        let turn = ScopedModelOverrideSummary {
            id: ScopedModelOverrideId::new(uuid(10)),
            kind: ScopedModelOverrideKind::FiniteSwitchTurn {
                request_id: SwitchTurnRequestId::new(uuid(11)),
                duration: FiniteScopedTurnDuration::OneTurn,
            },
            target_model: ModelId::new("turn-model"),
            topology_epoch: TopologyEpoch(1),
        };
        let operation = ScopedModelOverrideSummary {
            id: ScopedModelOverrideId::new(uuid(20)),
            kind: ScopedModelOverrideKind::ImageOperation {
                operation_id: ImageOperationId::new(uuid(21)),
            },
            target_model: ModelId::new("operation-model"),
            topology_epoch: TopologyEpoch(2),
        };

        let baseline_only =
            SessionModelRoutingStatus::new(ModelId::new("baseline"), None, None, None);
        assert_eq!(baseline_only.effective_model, ModelId::new("baseline"));

        let turn_active = SessionModelRoutingStatus::new(
            ModelId::new("baseline"),
            Some(turn.clone()),
            None,
            None,
        );
        assert_eq!(turn_active.effective_model, ModelId::new("turn-model"));

        let operation_active = SessionModelRoutingStatus::new(
            ModelId::new("baseline"),
            Some(turn),
            Some(operation),
            None,
        );
        assert_eq!(
            operation_active.effective_model,
            ModelId::new("operation-model")
        );
    }

    #[test]
    fn until_changed_is_not_encoded_as_zero_turns() {
        let duration = SwitchTurnDuration::UntilChanged;
        let json = serde_json::to_value(duration).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "duration_type": "until_changed" })
        );
        assert!(!json.to_string().contains("\"turns\":0"));

        let zero_turns = serde_json::json!({
            "duration_type": "finite",
            "duration": {
                "duration": "turns",
                "turns": 0
            }
        });
        assert!(serde_json::from_value::<SwitchTurnDuration>(zero_turns).is_err());
    }

    #[test]
    fn approval_parent_linkage_shapes_are_typed() {
        let switch = ModelRoutingApprovalRequest::SwitchTurn {
            request_id: SwitchTurnRequestId::new(uuid(30)),
            reason: SwitchTurnApprovalReason::UntilChangedFromModelOrigin,
        };
        let image = ModelRoutingApprovalRequest::ImageOperation {
            operation_id: ImageOperationId::new(uuid(31)),
            reason: ImageOperationApprovalReason::CrossProvider,
        };

        let switch_json = serde_json::to_value(&switch).unwrap();
        assert_eq!(switch_json["parent"], "switch_turn");
        assert!(switch_json.get("request_id").is_some());
        assert!(switch_json.get("operation_id").is_none());
        let parsed_switch: ModelRoutingApprovalRequest =
            serde_json::from_value(switch_json).unwrap();
        assert_eq!(parsed_switch, switch);

        let image_json = serde_json::to_value(&image).unwrap();
        assert_eq!(image_json["parent"], "image_operation");
        assert!(image_json.get("operation_id").is_some());
        assert!(image_json.get("request_id").is_none());
        let parsed_image: ModelRoutingApprovalRequest = serde_json::from_value(image_json).unwrap();
        assert_eq!(parsed_image, image);
    }

    #[test]
    fn lifecycle_and_override_variants_roundtrip() {
        let approval_id = ApprovalId::new();
        let phases = vec![
            ImageOperationPhase::AwaitingApproval { approval_id },
            ImageOperationPhase::Terminal {
                terminal: ImageOperationTerminalClass::ScopedRestoreFailed {
                    trigger: PostActivationImageTerminal::Cancelled,
                },
            },
        ];
        for phase in phases {
            let encoded = serde_json::to_string(&phase).unwrap();
            let decoded: ImageOperationPhase = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, phase);
        }

        let control = SwitchTurnControlResult::Denied {
            request_id: SwitchTurnRequestId::new(uuid(61)),
            reason: SwitchTurnDenialReason::ScopedOverrideConflict,
        };
        let encoded = serde_json::to_string(&control).unwrap();
        let decoded: SwitchTurnControlResult = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, control);

        let switch_phase = SwitchTurnPhase::Terminal {
            terminal: SwitchTurnTerminalClass::RestoreFailed {
                trigger: SwitchTurnRestoreTrigger::Interrupted,
            },
        };
        let encoded = serde_json::to_string(&switch_phase).unwrap();
        let decoded: SwitchTurnPhase = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, switch_phase);

        let approval_phase = ModelRoutingApprovalPhase::Terminal {
            terminal: ModelRoutingApprovalTerminalClass::SurfaceDetached,
        };
        let encoded = serde_json::to_string(&approval_phase).unwrap();
        let decoded: ModelRoutingApprovalPhase = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, approval_phase);

        let override_state = ScopedModelOverride {
            id: ScopedModelOverrideId::new(uuid(62)),
            kind: ScopedModelOverrideKind::FiniteSwitchTurn {
                request_id: SwitchTurnRequestId::new(uuid(63)),
                duration: FiniteScopedTurnDuration::Turns {
                    turns: NonZeroU32::new(3).unwrap(),
                },
            },
            previous_effective_model: ModelId::new("old-effective"),
            baseline_model_snapshot: ModelId::new("baseline"),
            target_model: ModelId::new("target"),
            topology_epoch: TopologyEpoch(7),
        };
        let encoded = serde_json::to_string(&override_state).unwrap();
        let decoded: ScopedModelOverride = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, override_state);
    }

    #[test]
    fn phase0_declared_enum_variants_roundtrip() {
        roundtrip(PromptSource::UserProvided {
            message_id: MessageId::new(uuid(70)),
        });
        roundtrip(PromptSource::ModelDistilled {
            tool_call_id: ToolCallId::new("tool-call-70"),
        });
        roundtrip(PromptSource::Hybrid {
            user_message_ids: vec![MessageId::new(uuid(71)), MessageId::new(uuid(72))],
            tool_call_id: ToolCallId::new("tool-call-71"),
        });

        roundtrip(ImageSourceRef::Blob {
            blob_ref: BlobRef {
                blob_id: BlobId::new("blob-70"),
                media_type: "image/png".into(),
            },
        });
        roundtrip(ImageSourceRef::TranscriptBlock {
            message_id: MessageId::new(uuid(73)),
            block_id: BlockId::new(uuid(74)),
        });
        roundtrip(ImageSourceRef::AssistantImage {
            image_id: AssistantImageId::new(uuid(75)),
        });
        roundtrip(ImageSourceRef::ProviderNative {
            provider: ProviderId::new("gemini"),
            handle: ProviderImageHandle::new("provider-image-handle"),
            continuity: ImageContinuityDisposition::Available {
                continuity_ref: ImageContinuityRef::new("continuity-70"),
            },
            fallback_blob_ref: BlobRef {
                blob_id: BlobId::new("fallback-70"),
                media_type: "image/png".into(),
            },
        });
        roundtrip(ImageContinuityDisposition::NotProvided);
        roundtrip(ImageContinuityDisposition::UnsupportedBySourceProvider);

        roundtrip(ImageGenerationTargetPreference::Auto);
        roundtrip(ImageGenerationTargetPreference::ProviderDefault {
            provider: ProviderId::new("openai"),
        });
        roundtrip(ImageGenerationTargetPreference::Model {
            provider: ProviderId::new("gemini"),
            model: ModelId::new("provider-native-image-model"),
        });

        roundtrip(ImageSizePreference::Auto);
        roundtrip(ImageSizePreference::Square1024);
        roundtrip(ImageSizePreference::Portrait1024x1536);
        roundtrip(ImageSizePreference::Landscape1536x1024);
        roundtrip(ImageSizePreference::Custom {
            width: NonZeroU32::new(640).unwrap(),
            height: NonZeroU32::new(480).unwrap(),
        });

        for quality in [
            ImageQualityPreference::Auto,
            ImageQualityPreference::Low,
            ImageQualityPreference::Medium,
            ImageQualityPreference::High,
        ] {
            roundtrip(quality);
        }
        for format in [
            ImageFormatPreference::Auto,
            ImageFormatPreference::Png,
            ImageFormatPreference::Jpeg,
            ImageFormatPreference::Webp,
        ] {
            roundtrip(format);
        }

        for support in [
            ImageContinuityTokenSupport::Unsupported,
            ImageContinuityTokenSupport::SameProviderOnly,
            ImageContinuityTokenSupport::CrossProvider,
        ] {
            roundtrip(support);
        }

        roundtrip(ProviderTextDisposition::NotEmitted);
        roundtrip(ProviderTextDisposition::UnsupportedByBackend);
        roundtrip(ProviderTextDisposition::Captured {
            text_artifact_ref: TextArtifactRef::new("text-70"),
        });

        roundtrip(RevisedPromptDisposition::NotRequested);
        roundtrip(RevisedPromptDisposition::UnsupportedByBackend);
        roundtrip(RevisedPromptDisposition::Unchanged);
        roundtrip(RevisedPromptDisposition::Revised {
            text: PromptText::new("revised prompt").unwrap(),
            source: RevisedPromptSource::MeerkatProjection,
        });
        roundtrip(RevisedPromptSource::Provider);
        roundtrip(RevisedPromptSource::MeerkatProjection);

        roundtrip(ImageGenerationWarning::ContinuityDegraded);
        roundtrip(ImageGenerationWarning::UnsupportedArtifactDropped);
        roundtrip(ImageGenerationWarning::ProviderReturnedFewerImages {
            requested: NonZeroU32::new(2).unwrap(),
            returned: NonZeroU32::new(1).unwrap(),
        });
        roundtrip(ImageGenerationWarning::ProviderExecutionFailed {
            message: "provider unavailable".into(),
        });
        roundtrip(ImageGenerationWarning::BlobCommitFailed {
            message: "blob store unavailable".into(),
        });
        roundtrip(ImageGenerationWarning::ProviderTextCaptureFailed {
            message: "text capture unavailable".into(),
        });

        roundtrip(ProviderImageMetadata::NotEmitted);
        roundtrip(ProviderImageMetadata::OpenAi(OpenAiImageMetadata {
            target_model: "provider-api-image-model".into(),
            response_id: Some("resp-70".into()),
            image_generation_call_id: Some("ig-70".into()),
        }));
        roundtrip(ProviderImageMetadata::Gemini(GeminiImageMetadata {
            target_model: "provider-native-image-model".into(),
            response_id: Some("gemini-70".into()),
            continuity_ref: Some(ImageContinuityRef::new("continuity-71")),
        }));

        for phase in [
            ImageOperationPhase::Requested,
            ImageOperationPhase::Validating,
            ImageOperationPhase::PlanResolved,
            ImageOperationPhase::ProjectionSnapshotted,
            ImageOperationPhase::ScopedOverrideActive,
            ImageOperationPhase::ProviderCallInFlight,
            ImageOperationPhase::ProviderResultCaptured,
            ImageOperationPhase::BlobCommitPending,
            ImageOperationPhase::ResultCommitted,
            ImageOperationPhase::RestoringScopedOverride,
        ] {
            roundtrip(phase);
        }
        roundtrip(ImageOperationPhase::AwaitingApproval {
            approval_id: ApprovalId::new(),
        });
        roundtrip(ImageOperationPhase::Terminal {
            terminal: ImageOperationTerminalClass::Denied {
                reason: ImageOperationDenialReason::UnsupportedTarget,
            },
        });

        roundtrip(ImageOperationTerminalClass::Generated);
        roundtrip(ImageOperationTerminalClass::EmptyResult {
            provider_text: ProviderTextDisposition::NotEmitted,
        });
        roundtrip(ImageOperationTerminalClass::RefusedByProvider);
        roundtrip(ImageOperationTerminalClass::SafetyFiltered);
        roundtrip(ImageOperationTerminalClass::Failed);
        roundtrip(ImageOperationTerminalClass::Cancelled);
        roundtrip(ImageOperationTerminalClass::Timeout);
        roundtrip(ImageOperationTerminalClass::ScopedRestoreFailed {
            trigger: PostActivationImageTerminal::Failed,
        });

        for reason in [
            ImageOperationDenialReason::UnsupportedTarget,
            ImageOperationDenialReason::UnsupportedCount,
            ImageOperationDenialReason::CapabilityPolicy,
            ImageOperationDenialReason::CostPolicy,
            ImageOperationDenialReason::SafetyPolicy,
            ImageOperationDenialReason::ApprovalRequiredButUnavailable,
            ImageOperationDenialReason::ScopedOverrideConflict,
            ImageOperationDenialReason::RealtimeTransportConflict,
            ImageOperationDenialReason::ProjectionUnsupported,
        ] {
            roundtrip(reason);
        }
        roundtrip(ImageOperationDenialReason::DeniedDuringApproval {
            approvable: ImageOperationApprovalReason::CostExceedsThreshold,
        });

        for terminal in [
            PostActivationImageTerminal::Generated,
            PostActivationImageTerminal::EmptyResult,
            PostActivationImageTerminal::RefusedByProvider,
            PostActivationImageTerminal::SafetyFiltered,
            PostActivationImageTerminal::Failed,
            PostActivationImageTerminal::Cancelled,
            PostActivationImageTerminal::Timeout,
        ] {
            roundtrip(terminal);
        }
        roundtrip(PostActivationImageTerminal::Denied {
            reason: PostActivationImageDenialReason::ProjectionUnsupported,
        });
        for reason in [
            PostActivationImageDenialReason::CostPolicy,
            PostActivationImageDenialReason::SafetyPolicy,
            PostActivationImageDenialReason::ScopedOverrideConflict,
            PostActivationImageDenialReason::RealtimeTransportConflict,
            PostActivationImageDenialReason::ProjectionUnsupported,
        ] {
            roundtrip(reason);
        }
        roundtrip(PostActivationImageDenialReason::DeniedDuringApproval {
            approvable: ImageOperationApprovalReason::SafetyHold,
        });

        roundtrip(SwitchTurnDuration::Finite {
            duration: FiniteScopedTurnDuration::OneTurn,
        });
        roundtrip(SwitchTurnDuration::Finite {
            duration: FiniteScopedTurnDuration::Turns {
                turns: NonZeroU32::new(2).unwrap(),
            },
        });
        roundtrip(SwitchTurnDuration::UntilChanged);

        roundtrip(SwitchTurnOrigin::User {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        });
        roundtrip(SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::Provided {
                reason: SwitchTurnReasonText::new("use a stronger model").unwrap(),
            },
        });
        roundtrip(SwitchTurnOrigin::SystemPolicy {
            reason: SwitchTurnPolicyReason::SafetyHandoff,
        });
        for reason in [
            SwitchTurnPolicyReason::BudgetDowngrade,
            SwitchTurnPolicyReason::SafetyHandoff,
            SwitchTurnPolicyReason::ReleaseTemporaryHold,
        ] {
            roundtrip(reason);
        }

        roundtrip(SwitchTurnControlResult::Applied {
            request_id: SwitchTurnRequestId::new(uuid(80)),
            target_model: ModelId::new("gpt-5.5"),
            duration: SwitchTurnDuration::Finite {
                duration: FiniteScopedTurnDuration::OneTurn,
            },
        });
        roundtrip(SwitchTurnControlResult::AwaitingApproval {
            approval_id: ApprovalId::new(),
            target_model: ModelId::new("claude-opus-4-7"),
            duration: SwitchTurnDuration::UntilChanged,
            reason: SwitchTurnApprovalReason::UntilChangedFromModelOrigin,
        });
        roundtrip(SwitchTurnControlResult::Denied {
            request_id: SwitchTurnRequestId::new(uuid(83)),
            reason: SwitchTurnDenialReason::RealtimeTransportConflict,
        });

        for reason in [
            SwitchTurnDenialReason::UnsupportedModel,
            SwitchTurnDenialReason::CapabilityPolicy,
            SwitchTurnDenialReason::CostPolicy,
            SwitchTurnDenialReason::SafetyPolicy,
            SwitchTurnDenialReason::ApprovalRequiredButUnavailable,
            SwitchTurnDenialReason::ScopedOverrideConflict,
            SwitchTurnDenialReason::RealtimeTransportConflict,
            SwitchTurnDenialReason::ProjectionUnsupported,
        ] {
            roundtrip(reason);
        }
        roundtrip(SwitchTurnDenialReason::DeniedDuringApproval {
            approvable: SwitchTurnApprovalReason::RealtimeDetachRequired,
        });

        for reason in [
            SwitchTurnApprovalReason::CrossProvider,
            SwitchTurnApprovalReason::CostExceedsThreshold,
            SwitchTurnApprovalReason::SafetyHold,
            SwitchTurnApprovalReason::UntilChangedFromModelOrigin,
            SwitchTurnApprovalReason::RealtimeDetachRequired,
        ] {
            roundtrip(reason);
        }

        for reason in [
            ImageOperationApprovalReason::CrossProvider,
            ImageOperationApprovalReason::CostExceedsThreshold,
            ImageOperationApprovalReason::SafetyHold,
            ImageOperationApprovalReason::RealtimeDetachRequired,
        ] {
            roundtrip(reason);
        }

        for phase in [
            SwitchTurnPhase::Requested,
            SwitchTurnPhase::Validating,
            SwitchTurnPhase::PendingForBoundary,
            SwitchTurnPhase::ApplyingFiniteOverride,
            SwitchTurnPhase::ApplyingPersistentReconfigure,
            SwitchTurnPhase::ActiveFiniteOverride,
            SwitchTurnPhase::RestoringFiniteOverride,
        ] {
            roundtrip(phase);
        }
        roundtrip(SwitchTurnPhase::AwaitingApproval {
            approval_id: ApprovalId::new(),
        });
        roundtrip(SwitchTurnPhase::Terminal {
            terminal: SwitchTurnTerminalClass::ConsumedAndRestored,
        });

        for terminal in [
            SwitchTurnTerminalClass::ConsumedAndRestored,
            SwitchTurnTerminalClass::InterruptedAndRestored,
            SwitchTurnTerminalClass::PersistentReconfigureApplied,
            SwitchTurnTerminalClass::PersistentReconfigureFailed,
        ] {
            roundtrip(terminal);
        }
        roundtrip(SwitchTurnTerminalClass::Denied {
            reason: SwitchTurnDenialReason::UnsupportedModel,
        });
        roundtrip(SwitchTurnTerminalClass::RestoreFailed {
            trigger: SwitchTurnRestoreTrigger::Consumed,
        });
        roundtrip(SwitchTurnTerminalClass::RestoreFailed {
            trigger: SwitchTurnRestoreTrigger::Interrupted,
        });

        roundtrip(ModelRoutingApprovalPhase::Pending);
        roundtrip(ModelRoutingApprovalPhase::PresentedToUser);
        for terminal in [
            ModelRoutingApprovalTerminalClass::Approved,
            ModelRoutingApprovalTerminalClass::DeniedByUser,
            ModelRoutingApprovalTerminalClass::Expired,
            ModelRoutingApprovalTerminalClass::Interrupted,
            ModelRoutingApprovalTerminalClass::SessionArchived,
            ModelRoutingApprovalTerminalClass::SurfaceDetached,
        ] {
            roundtrip(ModelRoutingApprovalPhase::Terminal { terminal });
            roundtrip(terminal);
        }
        roundtrip(ModelRoutingApprovalRequest::SwitchTurn {
            request_id: SwitchTurnRequestId::new(uuid(84)),
            reason: SwitchTurnApprovalReason::CrossProvider,
        });
        roundtrip(ModelRoutingApprovalRequest::ImageOperation {
            operation_id: ImageOperationId::new(uuid(81)),
            reason: ImageOperationApprovalReason::RealtimeDetachRequired,
        });
        roundtrip(ScopedModelOverrideKind::ImageOperation {
            operation_id: ImageOperationId::new(uuid(82)),
        });
        roundtrip(ScopedModelOverrideKind::FiniteSwitchTurn {
            request_id: SwitchTurnRequestId::new(uuid(85)),
            duration: FiniteScopedTurnDuration::Turns {
                turns: NonZeroU32::new(3).unwrap(),
            },
        });
    }

    #[test]
    fn assistant_block_image_roundtrip() {
        let block = crate::types::AssistantBlock::Image {
            image_id: AssistantImageId::new(uuid(40)),
            blob_ref: BlobRef {
                blob_id: BlobId::new("generated-image"),
                media_type: "image/png".to_string(),
            },
            media_type: MediaType::new("image/png"),
            width: 1024,
            height: 1024,
            revised_prompt: RevisedPromptDisposition::Revised {
                text: PromptText::new("a red bridge at sunset").unwrap(),
                source: RevisedPromptSource::Provider,
            },
            meta: ProviderImageMetadata::OpenAi(OpenAiImageMetadata {
                target_model: "provider-api-image-model".to_string(),
                response_id: Some("resp_123".to_string()),
                image_generation_call_id: Some("ig_123".to_string()),
            }),
        };

        let json = serde_json::to_string(&block).unwrap();
        let parsed: crate::types::AssistantBlock = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, block);
    }
}
