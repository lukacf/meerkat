//! OpenAI-owned image generation planning.

use std::num::NonZeroU32;

use meerkat_core::Provider;
use meerkat_core::image_generation::{
    GenerateImageExecutionPlan, GenerateImageRequest, ImageFormatPreference,
    ImageGenerationBackendKind, ImageGenerationIntent, ImageGenerationProviderProfile,
    ImageGenerationProviderResolution, ImageGenerationTargetCapabilities,
    ImageOperationDenialReason, ImageOperationId, ImageQualityPreference, ImageSizePreference,
    ProviderId,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::model_profile::catalog::{
    ImageGenerationModelProfile, ImageGenerationModelRoute, OpenAiImageGenerationRequestShape,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiResponsesImagePlan {
    pub tool_name: String,
    pub model: ModelId,
    #[serde(default)]
    pub output: OpenAiImageOutputOptions,
    #[serde(default)]
    pub provider_params: OpenAiImageProviderParams,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiImagesApiPlan {
    pub endpoint: OpenAiImagesApiEndpoint,
    pub request_shape: OpenAiImagesApiRequestShape,
    #[serde(default)]
    pub output: OpenAiImageOutputOptions,
    #[serde(default)]
    pub provider_params: OpenAiImageProviderParams,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAiImageOutputOptions {
    pub size: OpenAiImageSize,
    pub quality: OpenAiImageQuality,
    pub output_format: OpenAiImageOutputFormat,
}

impl Default for OpenAiImageOutputOptions {
    fn default() -> Self {
        Self {
            size: OpenAiImageSize::Square1024,
            quality: OpenAiImageQuality::Auto,
            output_format: OpenAiImageOutputFormat::Png,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageSize {
    Auto,
    Square1024,
    Portrait1024x1536,
    Landscape1536x1024,
    Custom {
        width: NonZeroU32,
        height: NonZeroU32,
    },
}

impl OpenAiImageSize {
    pub fn as_wire_value(&self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Square1024 => "1024x1024".to_string(),
            Self::Portrait1024x1536 => "1024x1536".to_string(),
            Self::Landscape1536x1024 => "1536x1024".to_string(),
            Self::Custom { width, height } => format!("{width}x{height}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageQuality {
    Auto,
    Low,
    Medium,
    High,
}

impl OpenAiImageQuality {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageOutputFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiImageProviderParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<OpenAiImageBackground>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_compression: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moderation: Option<OpenAiImageModeration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<OpenAiImageAction>,
}

impl OpenAiImageProviderParams {
    fn is_empty(&self) -> bool {
        self.background.is_none()
            && self.output_compression.is_none()
            && self.moderation.is_none()
            && self.action.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageBackground {
    Auto,
    Transparent,
    Opaque,
}

impl OpenAiImageBackground {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Transparent => "transparent",
            Self::Opaque => "opaque",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageModeration {
    Auto,
    Low,
}

impl OpenAiImageModeration {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImageAction {
    Auto,
    Generate,
    Edit,
}

impl OpenAiImageAction {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Generate => "generate",
            Self::Edit => "edit",
        }
    }
}

impl OpenAiImageOutputFormat {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImagesApiEndpoint {
    Generations,
    Edits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiImagesApiRequestShape {
    GptImage,
    DallE,
}

impl From<OpenAiImageGenerationRequestShape> for OpenAiImagesApiRequestShape {
    fn from(value: OpenAiImageGenerationRequestShape) -> Self {
        match value {
            OpenAiImageGenerationRequestShape::GptImage => Self::GptImage,
            OpenAiImageGenerationRequestShape::DallE => Self::DallE,
        }
    }
}

#[derive(Debug)]
pub struct OpenAiImageGenerationProfile;

impl ImageGenerationProviderProfile for OpenAiImageGenerationProfile {
    fn canonical_provider(&self) -> Provider {
        Provider::OpenAI
    }

    fn image_generation_documentation(&self) -> Option<&'static str> {
        Some(
            r#"OpenAI:
- Models: provider:"openai" uses the catalog OpenAI image default; supported image targets and their hosted-tool vs Images API routes are owned by the shared model catalog.
- Use Meerkat's universal size, quality, format, count, and intent fields for normal requests; the adapter lowers format to OpenAI output_format.
- provider_params is only for advanced OpenAI-specific overrides. For the current hosted gpt-image-2 route, public callers should normally use {"background":"auto"|"opaque","output_compression":0..100,"moderation":"auto"|"low","action":"auto"|"generate"|"edit"}.
- action applies only to the hosted Responses image tool and is usually omitted in favor of the top-level Meerkat intent. Images API requests reject action.
- background:"transparent" is model-dependent; gpt-image-2 rejects it."#,
        )
    }

    fn resolve_execution_plan(
        &self,
        _operation_id: ImageOperationId,
        model: &ImageGenerationModelProfile,
        request: &GenerateImageRequest,
        capabilities: ImageGenerationTargetCapabilities,
        max_count: NonZeroU32,
    ) -> Result<ImageGenerationProviderResolution, ImageOperationDenialReason> {
        if model.provider != self.canonical_provider() {
            return Err(ImageOperationDenialReason::UnsupportedTarget);
        }

        let output = openai_image_output_options(request);
        let provider_params = openai_image_provider_params(request)?;

        if let ImageGenerationModelRoute::OpenAiImagesApi { request_shape } = model.route {
            if matches!(request.intent, ImageGenerationIntent::Edit { .. })
                || matches!(
                    &request.intent,
                    ImageGenerationIntent::Generate {
                        reference_images,
                        ..
                    } if !reference_images.is_empty()
                )
            {
                return Err(ImageOperationDenialReason::ProjectionUnsupported);
            }
            if provider_params.action.is_some() {
                return Err(ImageOperationDenialReason::ProjectionUnsupported);
            }
            if matches!(request_shape, OpenAiImageGenerationRequestShape::DallE)
                && !provider_params.is_empty()
            {
                return Err(ImageOperationDenialReason::ProjectionUnsupported);
            }
            let provider_plan = serde_json::to_value(OpenAiImagesApiPlan {
                endpoint: OpenAiImagesApiEndpoint::Generations,
                request_shape: request_shape.into(),
                output,
                provider_params,
            })
            .map_err(|_| ImageOperationDenialReason::ProjectionUnsupported)?;
            return Ok(ImageGenerationProviderResolution {
                provider_call_model: ModelId::new(model.model_id),
                execution_plan: GenerateImageExecutionPlan {
                    provider: ProviderId::new(self.canonical_provider().as_str()),
                    backend: ImageGenerationBackendKind::ProviderApi,
                    max_count,
                    capabilities,
                    requires_scoped_override: false,
                    provider_plan,
                },
            });
        }

        let ImageGenerationModelRoute::OpenAiHostedResponsesTool {
            provider_call_model_id,
            tool_model_id,
        } = model.route
        else {
            return Err(ImageOperationDenialReason::UnsupportedTarget);
        };
        let provider_call_model = provider_call_model_id
            .map(ModelId::new)
            .unwrap_or_else(|| ModelId::new(model.model_id));
        let provider_plan = serde_json::to_value(OpenAiResponsesImagePlan {
            tool_name: "image_generation".to_string(),
            model: ModelId::new(tool_model_id),
            output,
            provider_params,
        })
        .map_err(|_| ImageOperationDenialReason::ProjectionUnsupported)?;
        Ok(ImageGenerationProviderResolution {
            provider_call_model,
            execution_plan: GenerateImageExecutionPlan {
                provider: ProviderId::new(self.canonical_provider().as_str()),
                backend: ImageGenerationBackendKind::HostedTool,
                max_count,
                capabilities,
                requires_scoped_override: false,
                provider_plan,
            },
        })
    }
}

pub fn openai_image_output_options(request: &GenerateImageRequest) -> OpenAiImageOutputOptions {
    let size = match &request.size {
        ImageSizePreference::Auto => OpenAiImageSize::Square1024,
        ImageSizePreference::Square1024 => OpenAiImageSize::Square1024,
        ImageSizePreference::Portrait1024x1536 => OpenAiImageSize::Portrait1024x1536,
        ImageSizePreference::Landscape1536x1024 => OpenAiImageSize::Landscape1536x1024,
        ImageSizePreference::Custom { width, height } => OpenAiImageSize::Custom {
            width: *width,
            height: *height,
        },
    };
    let quality = match request.quality {
        ImageQualityPreference::Auto => OpenAiImageQuality::Auto,
        ImageQualityPreference::Low => OpenAiImageQuality::Low,
        ImageQualityPreference::Medium => OpenAiImageQuality::Medium,
        ImageQualityPreference::High => OpenAiImageQuality::High,
    };
    let output_format = match request.format {
        ImageFormatPreference::Auto | ImageFormatPreference::Png => OpenAiImageOutputFormat::Png,
        ImageFormatPreference::Jpeg => OpenAiImageOutputFormat::Jpeg,
        ImageFormatPreference::Webp => OpenAiImageOutputFormat::Webp,
    };
    OpenAiImageOutputOptions {
        size,
        quality,
        output_format,
    }
}

fn openai_image_provider_params(
    request: &GenerateImageRequest,
) -> Result<OpenAiImageProviderParams, ImageOperationDenialReason> {
    let Some(value) = request.provider_params.as_ref() else {
        return Ok(OpenAiImageProviderParams::default());
    };
    let params: OpenAiImageProviderParams = serde_json::from_value(value.clone())
        .map_err(|_| ImageOperationDenialReason::ProjectionUnsupported)?;
    if params
        .output_compression
        .is_some_and(|compression| compression > 100)
    {
        return Err(ImageOperationDenialReason::ProjectionUnsupported);
    }
    Ok(params)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::image_generation::{
        ImageGenerationTargetPreference, PromptSource, PromptText, ToolCallId,
    };
    use meerkat_core::model_profile::catalog::{
        default_image_generation_model, image_generation_model,
    };
    use serde_json::json;

    fn capabilities() -> ImageGenerationTargetCapabilities {
        ImageGenerationTargetCapabilities {
            hosted_image_generation_tool: true,
            native_image_output: false,
            custom_tools: true,
            image_search_grounding: false,
            image_continuity_tokens:
                meerkat_core::image_generation::ImageContinuityTokenSupport::Unsupported,
        }
    }

    fn generate_request(
        provider_params: serde_json::Value,
    ) -> Result<GenerateImageRequest, Box<dyn std::error::Error>> {
        Ok(GenerateImageRequest::with_provider_params(
            ImageGenerationIntent::Generate {
                prompt: PromptText::new("draw a cat")?,
                prompt_source: PromptSource::ModelDistilled {
                    tool_call_id: ToolCallId::new("tool-call"),
                },
                reference_images: Vec::new(),
            },
            ImageGenerationTargetPreference::ProviderDefault {
                provider: ProviderId::new("openai"),
            },
            ImageSizePreference::Square1024,
            ImageQualityPreference::Auto,
            ImageFormatPreference::Png,
            NonZeroU32::MIN,
            Some(provider_params),
        )?)
    }

    fn operation_id() -> Result<ImageOperationId, serde_json::Error> {
        serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"")
    }

    fn openai_image_profile(model: &str) -> ImageGenerationModelProfile {
        image_generation_model(Provider::OpenAI, model)
            .unwrap_or_else(|| panic!("OpenAI image profile for {model} must be cataloged"))
    }

    #[test]
    fn profile_carries_openai_provider_params_in_private_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({
            "background": "opaque",
            "output_compression": 80,
            "moderation": "low",
            "action": "generate"
        }))?;
        let profile = default_image_generation_model(Provider::OpenAI)
            .expect("OpenAI default image profile must be cataloged");

        let resolution = OpenAiImageGenerationProfile
            .resolve_execution_plan(
                operation_id()?,
                &profile,
                &request,
                capabilities(),
                NonZeroU32::MIN,
            )
            .map_err(|err| std::io::Error::other(format!("resolve plan: {err:?}")))?;

        let plan: OpenAiResponsesImagePlan =
            serde_json::from_value(resolution.execution_plan.provider_plan)?;
        assert_eq!(
            plan.provider_params.background,
            Some(OpenAiImageBackground::Opaque)
        );
        assert_eq!(plan.provider_params.output_compression, Some(80));
        assert_eq!(
            plan.provider_params.moderation,
            Some(OpenAiImageModeration::Low)
        );
        assert_eq!(
            plan.provider_params.action,
            Some(OpenAiImageAction::Generate)
        );
        Ok(())
    }

    #[test]
    fn profile_rejects_invalid_openai_provider_params() -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({"output_compression": 101}))?;
        let profile = default_image_generation_model(Provider::OpenAI)
            .expect("OpenAI default image profile must be cataloged");

        let result = OpenAiImageGenerationProfile.resolve_execution_plan(
            operation_id()?,
            &profile,
            &request,
            capabilities(),
            NonZeroU32::MIN,
        );

        assert!(matches!(
            result,
            Err(ImageOperationDenialReason::ProjectionUnsupported)
        ));
        Ok(())
    }

    #[test]
    fn profile_routes_known_images_api_model_from_explicit_mapping()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({}))?;
        let profile = openai_image_profile("gpt-image-1");

        let resolution = OpenAiImageGenerationProfile
            .resolve_execution_plan(
                operation_id()?,
                &profile,
                &request,
                capabilities(),
                NonZeroU32::MIN,
            )
            .map_err(|err| std::io::Error::other(format!("resolve plan: {err:?}")))?;

        assert_eq!(resolution.provider_call_model, ModelId::new("gpt-image-1"));
        assert_eq!(
            resolution.execution_plan.backend,
            ImageGenerationBackendKind::ProviderApi
        );
        let plan: OpenAiImagesApiPlan =
            serde_json::from_value(resolution.execution_plan.provider_plan)?;
        assert_eq!(plan.endpoint, OpenAiImagesApiEndpoint::Generations);
        assert_eq!(plan.request_shape, OpenAiImagesApiRequestShape::GptImage);
        Ok(())
    }

    #[test]
    fn profile_routes_catalog_openai_text_model_to_hosted_tool()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({}))?;
        let profile = openai_image_profile("gpt-5.4");

        let resolution = OpenAiImageGenerationProfile
            .resolve_execution_plan(
                operation_id()?,
                &profile,
                &request,
                capabilities(),
                NonZeroU32::MIN,
            )
            .map_err(|err| std::io::Error::other(format!("resolve plan: {err:?}")))?;

        assert_eq!(resolution.provider_call_model, ModelId::new("gpt-5.4"));
        assert_eq!(
            resolution.execution_plan.backend,
            ImageGenerationBackendKind::HostedTool
        );
        Ok(())
    }

    #[test]
    fn profile_rejects_uncatalogued_image_like_model_name() {
        assert!(image_generation_model(Provider::OpenAI, "gpt-image-text-only").is_none());
    }
}
