//! SessionServiceRuntimeExt — v9 runtime extension for SessionService.
//!
//! This trait extends the existing SessionService with runtime-specific
//! operations. It lives in meerkat-runtime (NOT in core) to maintain
//! the separation: core owns SessionService, runtime owns runtime extensions.

use meerkat_core::lifecycle::InputId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::types::SessionId;

use crate::accept::AcceptOutcome;
use crate::completion::CompletionHandle;
use crate::input::Input;
use crate::input_state::StoredInputState;
use crate::meerkat_machine_types::{
    ImageOperationRoutingRequest, ImageOperationRoutingResult, RealtimeAttachmentStatus,
    SessionLlmReconfigureReport, SessionLlmReconfigureRequest, SwitchTurnRequest,
};
use crate::runtime_state::RuntimeState;
use crate::traits::{ResetReport, RetireReport, RuntimeDriverError};

/// Runtime mode for a session service instance.
///
/// This branch is runtime-backed only. All sessions use v9 runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeMode {
    /// Full v9 runtime-backed mode.
    V9Compliant,
}

/// v9 runtime extensions for SessionService.
///
/// Surfaces query this to decide whether to expose v9 runtime methods.
/// Legacy-mode surfaces MUST NOT advertise v9 capabilities.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait SessionServiceRuntimeExt: Send + Sync {
    /// Get the runtime mode.
    fn runtime_mode(&self) -> RuntimeMode;

    /// Accept an input for a session.
    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError>;

    /// Accept an input and optionally return a completion handle that resolves
    /// when the admitted work reaches a terminal runtime outcome.
    async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<CompletionHandle>), RuntimeDriverError>;

    /// Get the runtime state for a session.
    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError>;

    /// Get the live attachment status for a session.
    async fn realtime_attachment_status(
        &self,
        session_id: &SessionId,
    ) -> Result<RealtimeAttachmentStatus, RuntimeDriverError>;

    /// Wave-c C-9c R4: fully-projected public channel status. RPC / MCP
    /// `realtime/status` responders use this so `attempt_count` /
    /// `next_retry_at` / `deadline_at` come from DSL state projected off
    /// the reconnect overlay's DSL-projected progress, not hard-coded
    /// defaults.
    async fn realtime_channel_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_contracts::RealtimeChannelStatus, RuntimeDriverError>;

    /// Retire a session's runtime.
    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError>;

    /// Reset a session's runtime.
    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError>;

    /// Get the state of a specific input, bundled with its DSL-owned seed
    /// (phase / run association / boundary sequence).
    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError>;

    /// List all active (non-terminal) inputs for a session.
    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError>;

    /// Canonically reconfigure the LLM identity for a registered live session.
    async fn reconfigure_session_llm_identity(
        &self,
        session_id: &SessionId,
        request: SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError>;

    async fn configure_model_routing_baseline(
        &self,
        _session_id: &SessionId,
        _baseline_model: meerkat_core::lifecycle::run_primitive::ModelId,
        _realtime_capable: bool,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "model routing baseline is not supported by this runtime adapter".into(),
        ))
    }

    async fn session_model_routing_status(
        &self,
        _session_id: &SessionId,
    ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "model routing status is not supported by this runtime adapter".into(),
        ))
    }

    async fn resolve_image_generation_plan(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        request: &meerkat_core::image_generation::GenerateImageRequest,
    ) -> Result<
        Result<
            meerkat_core::image_generation::ImageGenerationResolvedPlan,
            meerkat_core::image_generation::ImageOperationDenialReason,
        >,
        RuntimeDriverError,
    > {
        let status = self.session_model_routing_status(session_id).await?;
        Ok(resolve_image_generation_plan_from_status(
            &status,
            operation_id,
            request,
        ))
    }

    async fn request_switch_turn(
        &self,
        _session_id: &SessionId,
        _request: SwitchTurnRequest,
    ) -> Result<meerkat_core::image_generation::SwitchTurnControlResult, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "switch_turn is not supported by this runtime adapter".into(),
        ))
    }

    async fn admit_model_routing_assistant_turn(
        &self,
        _session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "model routing turn admission is not supported by this runtime adapter".into(),
        ))
    }

    async fn begin_image_operation(
        &self,
        _session_id: &SessionId,
        _request: ImageOperationRoutingRequest,
    ) -> Result<ImageOperationRoutingResult, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation routing is not supported by this runtime adapter".into(),
        ))
    }

    async fn activate_image_operation_override(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation activation is not supported by this runtime adapter".into(),
        ))
    }

    async fn complete_image_operation(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
        _terminal: meerkat_core::image_generation::ImageOperationTerminalClass,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation completion is not supported by this runtime adapter".into(),
        ))
    }

    async fn restore_image_operation_override(
        &self,
        _session_id: &SessionId,
        _operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        Err(RuntimeDriverError::Internal(
            "image operation restore is not supported by this runtime adapter".into(),
        ))
    }
}

pub fn resolve_image_generation_plan_from_status(
    status: &meerkat_core::image_generation::SessionModelRoutingStatus,
    operation_id: meerkat_core::image_generation::ImageOperationId,
    request: &meerkat_core::image_generation::GenerateImageRequest,
) -> Result<
    meerkat_core::image_generation::ImageGenerationResolvedPlan,
    meerkat_core::image_generation::ImageOperationDenialReason,
> {
    use meerkat_core::image_generation::{
        GeminiImageTurnPlan, GenerateImageExecutionPlan, ImageContinuityTokenSupport,
        ImageGenerationIntent, ImageGenerationTargetCapabilities, ImageGenerationTargetPreference,
        ImageOperationDenialReason, OpenAiImagesApiEndpoint, OpenAiImagesApiPlan,
        OpenAiResponsesImagePlan, ProjectionSnapshotId,
    };
    let capabilities = ImageGenerationTargetCapabilities {
        hosted_image_generation_tool: true,
        native_image_output: true,
        custom_tools: false,
        image_search_grounding: false,
        image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
    };
    let one = std::num::NonZeroU32::MIN;
    let effective_provider =
        meerkat_core::Provider::infer_from_model(status.effective_model.as_str());
    let provider = match &request.target {
        ImageGenerationTargetPreference::Auto => match effective_provider {
            Some(meerkat_core::Provider::OpenAI) => "openai".to_string(),
            Some(meerkat_core::Provider::Gemini) => "gemini".to_string(),
            _ => return Err(ImageOperationDenialReason::UnsupportedTarget),
        },
        ImageGenerationTargetPreference::ProviderDefault { provider } => provider.0.clone(),
        ImageGenerationTargetPreference::Model { provider, .. } => provider.0.clone(),
    };
    let provider_key = provider.to_ascii_lowercase();
    let explicit_model = match &request.target {
        ImageGenerationTargetPreference::Model { model, .. } => Some(model.clone()),
        _ => None,
    };
    let provider_model = explicit_model.unwrap_or_else(|| match provider_key.as_str() {
        "gemini" | "google" => ModelId::new("gemini-3.1-flash-image-preview"),
        "openai" if matches!(effective_provider, Some(meerkat_core::Provider::OpenAI)) => {
            status.effective_model.clone()
        }
        "openai" => ModelId::new("gpt-image-1"),
        _ => status.effective_model.clone(),
    });
    if matches!(
        request.target,
        ImageGenerationTargetPreference::Model { .. }
    ) && !explicit_image_model_matches_provider(provider_key.as_str(), provider_model.as_str())
    {
        return Err(ImageOperationDenialReason::UnsupportedTarget);
    }
    let execution_plan = match provider_key.as_str() {
        "openai" => {
            let is_images_api = provider_model.as_str().starts_with("gpt-image")
                || provider_model.as_str().starts_with("dall-e");
            if is_images_api {
                if matches!(request.intent, ImageGenerationIntent::Edit { .. }) {
                    return Err(ImageOperationDenialReason::ProjectionUnsupported);
                }
                if matches!(
                    &request.intent,
                    ImageGenerationIntent::Generate {
                        reference_images,
                        ..
                    } if !reference_images.is_empty()
                ) {
                    return Err(ImageOperationDenialReason::ProjectionUnsupported);
                }
                GenerateImageExecutionPlan::OpenAiImagesApi {
                    model: provider_model.clone(),
                    max_count: one,
                    capabilities,
                    plan: OpenAiImagesApiPlan {
                        endpoint: OpenAiImagesApiEndpoint::Generations,
                    },
                }
            } else {
                GenerateImageExecutionPlan::OpenAiHostedResponsesImageTool {
                    max_count: one,
                    capabilities,
                    plan: OpenAiResponsesImagePlan {
                        tool_name: "image_generation".to_string(),
                    },
                }
            }
        }
        "gemini" | "google" => GenerateImageExecutionPlan::GeminiNativeImageModel {
            model: provider_model.clone(),
            max_count: one,
            capabilities,
            plan: GeminiImageTurnPlan {
                projection_snapshot_id: ProjectionSnapshotId::new(operation_id.0),
            },
        },
        _ => return Err(ImageOperationDenialReason::UnsupportedTarget),
    };
    if request.count > one {
        return Err(ImageOperationDenialReason::UnsupportedCount);
    }
    let projected_messages = image_projection_messages(request);
    Ok(
        meerkat_core::image_generation::ImageGenerationResolvedPlan {
            provider_model,
            machine_routing_model: status.effective_model.clone(),
            machine_routing_realtime_capable: true,
            execution_plan,
            projected_messages,
        },
    )
}

fn explicit_image_model_matches_provider(provider_key: &str, model: &str) -> bool {
    match meerkat_core::Provider::infer_from_model(model) {
        Some(meerkat_core::Provider::OpenAI) => provider_key == "openai",
        Some(meerkat_core::Provider::Gemini) => {
            provider_key == "gemini" || provider_key == "google"
        }
        Some(_) => false,
        None => match provider_key {
            "openai" => model.starts_with("gpt-image") || model.starts_with("dall-e"),
            "gemini" | "google" => model.starts_with("gemini-"),
            _ => false,
        },
    }
}

fn image_projection_messages(
    request: &meerkat_core::image_generation::GenerateImageRequest,
) -> Vec<meerkat_core::Message> {
    use meerkat_core::image_generation::ImageGenerationIntent;
    let text = match &request.intent {
        ImageGenerationIntent::Generate { prompt, .. } => prompt.content.clone(),
        ImageGenerationIntent::Edit { instruction, .. } => instruction.content.clone(),
    };
    vec![meerkat_core::Message::User(
        meerkat_core::UserMessage::text(text),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::image_generation::{
        GenerateImageExecutionPlan, GenerateImageRequest, ImageFormatPreference,
        ImageGenerationIntent, ImageGenerationTargetPreference, ImageOperationDenialReason,
        ImageOperationId, ImageQualityPreference, ImageSizePreference, PromptSource, PromptText,
        SessionModelRoutingStatus, ToolCallId,
    };
    use meerkat_core::{ProviderId, UserMessage};

    // Verify trait is object-safe
    fn _assert_object_safe(_: &dyn SessionServiceRuntimeExt) {}

    #[test]
    fn runtime_mode_equality() {
        assert_eq!(RuntimeMode::V9Compliant, RuntimeMode::V9Compliant);
    }

    fn image_request(provider: &str, model: &str) -> GenerateImageRequest {
        GenerateImageRequest::new(
            ImageGenerationIntent::Generate {
                prompt: PromptText::new("draw a tiny square").unwrap(),
                prompt_source: PromptSource::ModelDistilled {
                    tool_call_id: ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            ImageGenerationTargetPreference::Model {
                provider: ProviderId::new(provider),
                model: ModelId::new(model),
            },
            ImageSizePreference::Square1024,
            ImageQualityPreference::Low,
            ImageFormatPreference::Png,
            std::num::NonZeroU32::new(1).unwrap(),
        )
        .unwrap()
    }

    fn auto_image_request() -> GenerateImageRequest {
        GenerateImageRequest::new(
            ImageGenerationIntent::Generate {
                prompt: PromptText::new("draw a tiny square").unwrap(),
                prompt_source: PromptSource::ModelDistilled {
                    tool_call_id: ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            ImageGenerationTargetPreference::Auto,
            ImageSizePreference::Square1024,
            ImageQualityPreference::Low,
            ImageFormatPreference::Png,
            std::num::NonZeroU32::new(1).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn image_plan_keeps_provider_target_separate_from_machine_routing_model() {
        let status = SessionModelRoutingStatus::new(ModelId::new("gpt-5.4"), None, None, None);
        let operation_id = ImageOperationId::new(uuid::Uuid::nil());
        let plan = resolve_image_generation_plan_from_status(
            &status,
            operation_id,
            &image_request("openai", "gpt-image-1"),
        )
        .expect("openai image target should resolve");

        assert_eq!(plan.provider_model.as_str(), "gpt-image-1");
        assert_eq!(plan.machine_routing_model.as_str(), "gpt-5.4");
        assert!(matches!(
            plan.execution_plan,
            GenerateImageExecutionPlan::OpenAiImagesApi { .. }
        ));
        assert_eq!(plan.projected_messages.len(), 1);
        assert!(matches!(
            &plan.projected_messages[0],
            meerkat_core::Message::User(UserMessage { content, .. })
                if content[0].text_projection() == "draw a tiny square"
        ));
    }

    #[test]
    fn openai_images_api_rejects_unprojected_edit_and_reference_sources() {
        let status = SessionModelRoutingStatus::new(ModelId::new("gpt-5.4"), None, None, None);
        let mut request = image_request("openai", "gpt-image-1");
        let ImageGenerationIntent::Generate {
            reference_images, ..
        } = &mut request.intent
        else {
            panic!("expected generate request");
        };
        reference_images.push(
            meerkat_core::image_generation::ImageSourceRef::AssistantImage {
                image_id: meerkat_core::image_generation::AssistantImageId::new(uuid::Uuid::nil()),
            },
        );

        let denied = resolve_image_generation_plan_from_status(
            &status,
            ImageOperationId::new(uuid::Uuid::nil()),
            &request,
        )
        .expect_err("unhydrated OpenAI reference images should be denied");
        assert!(matches!(
            denied,
            ImageOperationDenialReason::ProjectionUnsupported
        ));
    }

    #[test]
    fn auto_image_plan_rejects_non_image_capable_session_providers() {
        let status =
            SessionModelRoutingStatus::new(ModelId::new("claude-sonnet-4-5"), None, None, None);
        let denied = resolve_image_generation_plan_from_status(
            &status,
            ImageOperationId::new(uuid::Uuid::nil()),
            &auto_image_request(),
        )
        .expect_err("auto image routing should not fabricate OpenAI for Anthropic sessions");

        assert!(matches!(
            denied,
            ImageOperationDenialReason::UnsupportedTarget
        ));
    }

    #[test]
    fn provider_default_openai_uses_known_image_default_when_session_model_is_not_openai() {
        let status =
            SessionModelRoutingStatus::new(ModelId::new("claude-sonnet-4-5"), None, None, None);
        let mut request = auto_image_request();
        request.target = ImageGenerationTargetPreference::ProviderDefault {
            provider: ProviderId::new("openai"),
        };
        let plan = resolve_image_generation_plan_from_status(
            &status,
            ImageOperationId::new(uuid::Uuid::nil()),
            &request,
        )
        .expect("provider-default OpenAI should choose a known image backend model");

        assert_eq!(plan.provider_model.as_str(), "gpt-image-1");
        assert_eq!(plan.machine_routing_model.as_str(), "claude-sonnet-4-5");
        assert!(matches!(
            plan.execution_plan,
            GenerateImageExecutionPlan::OpenAiImagesApi { .. }
        ));
    }

    #[test]
    fn explicit_image_plan_rejects_provider_model_mismatch() {
        let status = SessionModelRoutingStatus::new(ModelId::new("gpt-5.4"), None, None, None);
        let denied = resolve_image_generation_plan_from_status(
            &status,
            ImageOperationId::new(uuid::Uuid::nil()),
            &image_request("openai", "claude-sonnet-4-5"),
        )
        .expect_err("OpenAI target must not accept an Anthropic model");

        assert!(matches!(
            denied,
            ImageOperationDenialReason::UnsupportedTarget
        ));

        let denied = resolve_image_generation_plan_from_status(
            &status,
            ImageOperationId::new(uuid::Uuid::nil()),
            &image_request("gemini", "gpt-image-1"),
        )
        .expect_err("Gemini target must not accept an OpenAI image model");

        assert!(matches!(
            denied,
            ImageOperationDenialReason::UnsupportedTarget
        ));
    }
}
