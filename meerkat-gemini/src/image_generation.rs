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
use meerkat_core::model_profile::catalog::{
    ImageGenerationModelProfile, ImageGenerationModelRoute, ImageGenerationSizeParameter,
};
use serde::{Deserialize, Serialize};

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
    fn canonical_provider(&self) -> Provider {
        Provider::Gemini
    }

    fn provider_aliases(&self) -> &'static [&'static str] {
        &["google"]
    }

    fn image_generation_documentation(&self) -> Option<&'static str> {
        Some(
            r#"Gemini:
- Models: provider:"gemini" uses the catalog Gemini image default; supported native image models are owned by the shared model catalog.
- provider_params: {"aspect_ratio":"1:1"|"16:9"|"9:16"|"square1x1"|"landscape16x9"|"portrait9x16","image_size":"1K"|"2K"|"4K"|"one_k"|"two_k"|"four_k"}.
- Universal size maps to Gemini aspectRatio/imageSize first; provider_params override those mapped values."#,
        )
    }

    fn resolve_execution_plan(
        &self,
        operation_id: ImageOperationId,
        model: &ImageGenerationModelProfile,
        request: &GenerateImageRequest,
        capabilities: ImageGenerationTargetCapabilities,
        max_count: NonZeroU32,
    ) -> Result<ImageGenerationProviderResolution, ImageOperationDenialReason> {
        if model.provider != self.canonical_provider() {
            return Err(ImageOperationDenialReason::UnsupportedTarget);
        }
        let ImageGenerationModelRoute::GeminiNativeModel {
            image_size_parameter,
        } = model.route
        else {
            return Err(ImageOperationDenialReason::UnsupportedTarget);
        };
        let output = gemini_image_output_options_with_provider_params(
            image_size_parameter,
            &request.size,
            request.provider_params.as_ref(),
        )?;
        let provider_plan = serde_json::to_value(GeminiImageTurnPlan {
            projection_snapshot_id: ProjectionSnapshotId::new(operation_id.0),
            output,
        })
        .map_err(|_| ImageOperationDenialReason::ProjectionUnsupported)?;
        Ok(ImageGenerationProviderResolution {
            provider_call_model: ModelId::new(model.model_id),
            execution_plan: GenerateImageExecutionPlan {
                provider: ProviderId::new(self.canonical_provider().as_str()),
                backend: ImageGenerationBackendKind::NativeModel,
                max_count,
                capabilities,
                requires_scoped_override: true,
                provider_plan,
            },
        })
    }
}

pub fn gemini_image_output_options(
    image_size_parameter: ImageGenerationSizeParameter,
    size: &ImageSizePreference,
) -> GeminiImageOutputOptions {
    gemini_base_image_output_options(image_size_parameter, size)
}

fn gemini_image_output_options_with_provider_params(
    image_size_parameter: ImageGenerationSizeParameter,
    size: &ImageSizePreference,
    provider_params: Option<&serde_json::Value>,
) -> Result<GeminiImageOutputOptions, ImageOperationDenialReason> {
    let mut output = gemini_base_image_output_options(image_size_parameter, size);
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
    image_size_parameter: ImageGenerationSizeParameter,
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
    let image_size = match image_size_parameter {
        ImageGenerationSizeParameter::Unsupported => None,
        ImageGenerationSizeParameter::Supported => Some(match size {
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
        }),
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
    use meerkat_core::model_profile::catalog::{
        default_image_generation_model, image_generation_model,
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

    fn gemini_image_profile(model: &str) -> ImageGenerationModelProfile {
        image_generation_model(Provider::Gemini, model)
            .unwrap_or_else(|| panic!("Gemini image profile for {model} must be cataloged"))
    }

    #[test]
    fn profile_carries_gemini_provider_params_in_private_plan()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({
            "aspect_ratio": "16:9",
            "image_size": "2K"
        }))?;
        let profile = default_image_generation_model(Provider::Gemini)
            .expect("Gemini default image profile must be cataloged");

        let resolution = GeminiImageGenerationProfile
            .resolve_execution_plan(
                operation_id()?,
                &profile,
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
        let profile = default_image_generation_model(Provider::Gemini)
            .expect("Gemini default image profile must be cataloged");

        let result = GeminiImageGenerationProfile.resolve_execution_plan(
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
    fn catalog_rejects_non_image_gemini_models() {
        assert!(image_generation_model(Provider::Gemini, "gemini-3-flash-preview").is_none());
    }

    #[test]
    fn profile_uses_catalog_image_size_support() -> Result<(), Box<dyn std::error::Error>> {
        let request = generate_request(json!({}))?;
        let profile = gemini_image_profile("gemini-2.5-flash-image");

        let resolution = GeminiImageGenerationProfile
            .resolve_execution_plan(
                operation_id()?,
                &profile,
                &request,
                capabilities(),
                NonZeroU32::MIN,
            )
            .map_err(|err| std::io::Error::other(format!("resolve plan: {err:?}")))?;

        let plan: GeminiImageTurnPlan =
            serde_json::from_value(resolution.execution_plan.provider_plan)?;
        assert_eq!(plan.output.image_size, None);
        Ok(())
    }
}
