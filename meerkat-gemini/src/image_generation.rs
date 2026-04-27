//! Gemini-owned image generation planning.

use std::num::NonZeroU32;

use meerkat_core::Provider;
use meerkat_core::image_generation::{
    GenerateImageExecutionPlan, GenerateImageRequest, ImageGenerationBackendKind,
    ImageGenerationProviderProfile, ImageGenerationProviderResolution,
    ImageGenerationTargetCapabilities, ImageOperationDenialReason, ImageOperationId,
    ImageSizePreference, ProjectionSnapshotId, ProviderId,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use serde::{Deserialize, Serialize};

const GEMINI_IMAGE_DEFAULT_MODEL: &str = "gemini-3.1-flash-image-preview";
const GEMINI_IMAGE_MODELS: &[&str] = &[
    GEMINI_IMAGE_DEFAULT_MODEL,
    "gemini-3-pro-image-preview",
    "gemini-2.5-flash-image",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeminiImageTurnPlan {
    pub projection_snapshot_id: ProjectionSnapshotId,
    #[serde(default)]
    pub output: GeminiImageOutputOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeminiImageOutputOptions {
    pub aspect_ratio: GeminiImageAspectRatio,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<GeminiImageSize>,
}

impl Default for GeminiImageOutputOptions {
    fn default() -> Self {
        Self {
            aspect_ratio: GeminiImageAspectRatio::Square1x1,
            image_size: Some(GeminiImageSize::OneK),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeminiImageAspectRatio {
    #[serde(alias = "1:1")]
    Square1x1,
    #[serde(alias = "16:9")]
    Landscape16x9,
    #[serde(alias = "9:16")]
    Portrait9x16,
}

impl GeminiImageAspectRatio {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::Square1x1 => "1:1",
            Self::Landscape16x9 => "16:9",
            Self::Portrait9x16 => "9:16",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeminiImageSize {
    #[serde(alias = "1K", alias = "1k")]
    OneK,
    #[serde(alias = "2K", alias = "2k")]
    TwoK,
    #[serde(alias = "4K", alias = "4k")]
    FourK,
}

impl GeminiImageSize {
    pub fn as_wire_value(self) -> &'static str {
        match self {
            Self::OneK => "1K",
            Self::TwoK => "2K",
            Self::FourK => "4K",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeminiImageProviderParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<GeminiImageAspectRatio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_size: Option<GeminiImageSize>,
}

#[derive(Debug)]
pub struct GeminiImageGenerationProfile;

impl ImageGenerationProviderProfile for GeminiImageGenerationProfile {
    fn canonical_provider(&self) -> &'static str {
        "gemini"
    }

    fn provider_aliases(&self) -> &'static [&'static str] {
        &["google"]
    }

    fn default_model_for_session(
        &self,
        _effective_provider: Option<Provider>,
        _effective_model: &ModelId,
    ) -> ModelId {
        ModelId::new(GEMINI_IMAGE_DEFAULT_MODEL)
    }

    fn owns_model(&self, model: &str) -> bool {
        is_supported_gemini_image_model(model)
    }

    fn image_generation_documentation(&self) -> Option<&'static str> {
        Some(
            r#"Gemini:
- Models: provider:"gemini" uses the Gemini image default; pass provider plus model to force another Gemini-owned image model.
- provider_params: {"aspect_ratio":"1:1"|"16:9"|"9:16"|"square1x1"|"landscape16x9"|"portrait9x16","image_size":"1K"|"2K"|"4K"|"one_k"|"two_k"|"four_k"}.
- Universal size maps to Gemini aspectRatio/imageSize first; provider_params override those mapped values."#,
        )
    }

    fn resolve_execution_plan(
        &self,
        operation_id: ImageOperationId,
        requested_model: &ModelId,
        request: &GenerateImageRequest,
        capabilities: ImageGenerationTargetCapabilities,
        max_count: NonZeroU32,
    ) -> Result<ImageGenerationProviderResolution, ImageOperationDenialReason> {
        if !self.owns_model(requested_model.as_str()) {
            return Err(ImageOperationDenialReason::UnsupportedTarget);
        }
        let output = gemini_image_output_options_with_provider_params(
            requested_model.as_str(),
            &request.size,
            request.provider_params.as_ref(),
        )?;
        let provider_plan = serde_json::to_value(GeminiImageTurnPlan {
            projection_snapshot_id: ProjectionSnapshotId::new(operation_id.0),
            output,
        })
        .map_err(|_| ImageOperationDenialReason::ProjectionUnsupported)?;
        Ok(ImageGenerationProviderResolution {
            provider_call_model: requested_model.clone(),
            execution_plan: GenerateImageExecutionPlan {
                provider: ProviderId::new(self.canonical_provider()),
                backend: ImageGenerationBackendKind::NativeModel,
                max_count,
                capabilities,
                requires_scoped_override: true,
                provider_plan,
            },
        })
    }
}

fn is_supported_gemini_image_model(model: &str) -> bool {
    GEMINI_IMAGE_MODELS.contains(&model)
}

pub fn gemini_image_output_options(
    model: &str,
    size: &ImageSizePreference,
) -> GeminiImageOutputOptions {
    gemini_base_image_output_options(model, size)
}

fn gemini_image_output_options_with_provider_params(
    model: &str,
    size: &ImageSizePreference,
    provider_params: Option<&serde_json::Value>,
) -> Result<GeminiImageOutputOptions, ImageOperationDenialReason> {
    let mut output = gemini_base_image_output_options(model, size);
    if let Some(value) = provider_params {
        let params: GeminiImageProviderParams = serde_json::from_value(value.clone())
            .map_err(|_| ImageOperationDenialReason::ProjectionUnsupported)?;
        if let Some(aspect_ratio) = params.aspect_ratio {
            output.aspect_ratio = aspect_ratio;
        }
        if params.image_size.is_some() {
            output.image_size = params.image_size;
        }
    }
    Ok(output)
}

fn gemini_base_image_output_options(
    model: &str,
    size: &ImageSizePreference,
) -> GeminiImageOutputOptions {
    let aspect_ratio = match size {
        ImageSizePreference::Portrait1024x1536 => GeminiImageAspectRatio::Portrait9x16,
        ImageSizePreference::Landscape1536x1024 => GeminiImageAspectRatio::Landscape16x9,
        ImageSizePreference::Custom { width, height } if width > height => {
            GeminiImageAspectRatio::Landscape16x9
        }
        ImageSizePreference::Custom { width, height } if height > width => {
            GeminiImageAspectRatio::Portrait9x16
        }
        _ => GeminiImageAspectRatio::Square1x1,
    };
    let image_size = if model == "gemini-2.5-flash-image" {
        None
    } else {
        Some(match size {
            ImageSizePreference::Custom { width, height } => {
                let edge = width.get().max(height.get());
                if edge > 2048 {
                    GeminiImageSize::FourK
                } else if edge > 1024 {
                    GeminiImageSize::TwoK
                } else {
                    GeminiImageSize::OneK
                }
            }
            _ => GeminiImageSize::OneK,
        })
    };
    GeminiImageOutputOptions {
        aspect_ratio,
        image_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::image_generation::{
        ImageFormatPreference, ImageGenerationIntent, ImageGenerationTargetPreference,
        ImageQualityPreference, PromptSource, PromptText, ToolCallId,
    };
    use serde_json::json;

    fn capabilities() -> ImageGenerationTargetCapabilities {
        ImageGenerationTargetCapabilities {
            hosted_image_generation_tool: false,
            native_image_output: true,
            custom_tools: true,
            image_search_grounding: false,
            image_continuity_tokens:
                meerkat_core::image_generation::ImageContinuityTokenSupport::SameProviderOnly,
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
                provider: ProviderId::new("gemini"),
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

    #[test]
    fn profile_carries_gemini_provider_params_in_private_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({
            "aspect_ratio": "16:9",
            "image_size": "2K"
        }))?;

        let resolution = GeminiImageGenerationProfile
            .resolve_execution_plan(
                operation_id()?,
                &ModelId::new(GEMINI_IMAGE_DEFAULT_MODEL),
                &request,
                capabilities(),
                NonZeroU32::MIN,
            )
            .map_err(|err| std::io::Error::other(format!("resolve plan: {err:?}")))?;

        let plan: GeminiImageTurnPlan =
            serde_json::from_value(resolution.execution_plan.provider_plan)?;
        assert_eq!(
            plan.output.aspect_ratio,
            GeminiImageAspectRatio::Landscape16x9
        );
        assert_eq!(plan.output.image_size, Some(GeminiImageSize::TwoK));
        Ok(())
    }

    #[test]
    fn profile_rejects_invalid_gemini_provider_params() -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({"aspect_ratio": "21:9"}))?;

        let result = GeminiImageGenerationProfile.resolve_execution_plan(
            operation_id()?,
            &ModelId::new(GEMINI_IMAGE_DEFAULT_MODEL),
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
    fn profile_rejects_non_image_gemini_models() -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(serde_json::Value::Null)?;

        assert!(!GeminiImageGenerationProfile.owns_model("gemini-3-flash-preview"));
        let result = GeminiImageGenerationProfile.resolve_execution_plan(
            operation_id()?,
            &ModelId::new("gemini-3-flash-preview"),
            &request,
            capabilities(),
            NonZeroU32::MIN,
        );

        assert!(matches!(
            result,
            Err(ImageOperationDenialReason::UnsupportedTarget)
        ));
        Ok(())
    }
}
