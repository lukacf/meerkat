//! Builtin assistant image generation tool.

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use base64::Engine;
use meerkat_core::image_generation::{
    AssistantImageId, AssistantImageRef, GenerateImageRequest, ImageFormatPreference,
    ImageGenerationIntent, ImageGenerationPlanner, ImageGenerationResolvedPlan,
    ImageGenerationTargetPreference, ImageGenerationToolResult, ImageOperationDenialReason,
    ImageOperationId, ImageOperationPhase, ImageOperationTerminalClass, ImageQualityPreference,
    ImageSizePreference, ImageSourceRef, PostActivationImageDenialReason,
    PostActivationImageTerminal, PromptSource, PromptText, ProviderId, ProviderTextDisposition,
    TextArtifactRef, ToolCallId,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::ops::SessionEffect;
use meerkat_core::types::{AssistantBlock, ToolDef, ToolProvenance, ToolSourceKind};
use meerkat_core::{BlobStore, SessionId};
use meerkat_llm_core::{
    ImageGenerationExecutor, ProviderImageGenerationOutput, ProviderImageGenerationRequest,
};
use meerkat_runtime::{
    ImageOperationRoutingRequest, ImageOperationRoutingResult, ModelRoutingApprovalDisposition,
    ModelRoutingRealtimePolicy,
};
use meerkat_runtime::{RuntimeDriverError, SessionServiceRuntimeExt};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::num::NonZeroU32;
use std::sync::Arc;
use tracing::warn;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ImageGenerationMachine: Send + Sync {
    async fn session_model_routing_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError>;

    async fn begin_image_operation(
        &self,
        session_id: &SessionId,
        request: ImageOperationRoutingRequest,
    ) -> Result<ImageOperationRoutingResult, RuntimeDriverError>;

    async fn activate_image_operation_override(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
    ) -> Result<ImageOperationPhase, RuntimeDriverError>;

    async fn complete_image_operation(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
        terminal: ImageOperationTerminalClass,
    ) -> Result<ImageOperationPhase, RuntimeDriverError>;

    async fn restore_image_operation_override(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
    ) -> Result<ImageOperationPhase, RuntimeDriverError>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<T> ImageGenerationMachine for T
where
    T: SessionServiceRuntimeExt + ?Sized,
{
    async fn session_model_routing_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError> {
        SessionServiceRuntimeExt::session_model_routing_status(self, session_id).await
    }

    async fn begin_image_operation(
        &self,
        session_id: &SessionId,
        request: ImageOperationRoutingRequest,
    ) -> Result<ImageOperationRoutingResult, RuntimeDriverError> {
        SessionServiceRuntimeExt::begin_image_operation(self, session_id, request).await
    }

    async fn activate_image_operation_override(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
    ) -> Result<ImageOperationPhase, RuntimeDriverError> {
        SessionServiceRuntimeExt::activate_image_operation_override(self, session_id, operation_id)
            .await
    }

    async fn complete_image_operation(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
        terminal: ImageOperationTerminalClass,
    ) -> Result<ImageOperationPhase, RuntimeDriverError> {
        SessionServiceRuntimeExt::complete_image_operation(self, session_id, operation_id, terminal)
            .await
    }

    async fn restore_image_operation_override(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
    ) -> Result<ImageOperationPhase, RuntimeDriverError> {
        SessionServiceRuntimeExt::restore_image_operation_override(self, session_id, operation_id)
            .await
    }
}

#[derive(Clone)]
pub struct ImageGenerationToolRuntime {
    pub session_id: SessionId,
    pub machine: Arc<dyn ImageGenerationMachine>,
    pub planner: Arc<dyn ImageGenerationPlanner>,
    pub blob_store: Arc<dyn BlobStore>,
    pub executor: Arc<dyn ImageGenerationExecutor>,
}

#[derive(Clone)]
pub struct GenerateImageTool {
    runtime: ImageGenerationToolRuntime,
}

const GENERATE_IMAGE_TOOL_DOCUMENTATION: &str = r#"Generate or edit an assistant image through the session-owned image substrate.

Use this tool whenever the user asks you to generate, create, draw, render, or edit an image. If the user asks to save the result to disk, generate the image here first, then save the returned blob with `blob_save_file`. Do not use shell scripts, drawing libraries, or placeholder files as a substitute for requested image generation when this tool is available.

Use a simple request shape unless you explicitly need the canonical internal shape:
{"request":{"intent":"generate","prompt":"a cozy tabby cat by a sunlit window","size":"1024x1024","quality":"auto","format":"png","count":1}}

Routing and defaults:
- target defaults to "auto".
- On image-capable sessions, auto uses the current provider's registered image default.
- On non-image-capable session providers, auto is unsupported; set provider:"openai" or provider:"gemini".
- provider:"openai" or provider:"gemini" uses that provider's registered image default.
- To force a model, pass provider plus model. Passing only model is accepted when the catalog identifies a configured provider for that model.

Supported request fields:
- intent: "generate" for a new image, "edit" only with source_images. If omitted and prompt is present, intent defaults to "generate".
- prompt: text prompt for generation.
- instruction: edit instruction for edit requests.
- size: "auto", "1024x1024", "1024x1536", "1536x1024", or "WIDTHxHEIGHT". Size support is model/provider dependent; unsupported values may be rejected by the provider.
- quality: "auto", "low", "medium", or "high". Quality support is model/provider dependent.
- format: "auto", "png", "jpeg", "jpg", or "webp". Format support is model/provider dependent.
- provider_params: optional provider-specific JSON parameters documented by the selected provider profile.
- count/n: currently only 1 is supported.

Do not pass size as a bare top-level string outside request. Do not use intent:{type:"create"} unless you mean the compatibility alias for generate; prefer intent:"generate"."#;

impl GenerateImageTool {
    pub fn new(runtime: ImageGenerationToolRuntime) -> Self {
        Self { runtime }
    }

    fn description(&self) -> String {
        let provider_docs = self.runtime.planner.provider_documentation();
        if provider_docs.is_empty() {
            return GENERATE_IMAGE_TOOL_DOCUMENTATION.to_string();
        }

        format!(
            "{GENERATE_IMAGE_TOOL_DOCUMENTATION}\n\nConfigured provider image parameters:\n{}",
            provider_docs.join("\n\n")
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateImageToolArgs {
    #[schemars(
        with = "GenerateImageToolRequestSchema",
        description = "Image request. For normal generation, use {\"intent\":\"generate\",\"prompt\":\"...\",\"size\":\"1024x1024\",\"quality\":\"auto\",\"format\":\"png\",\"count\":1}. Omit optional fields to use automatic defaults. Provider/model and size behavior is described in the tool description."
    )]
    request: Value,
}

#[allow(dead_code)]
#[derive(Debug, JsonSchema)]
struct GenerateImageToolRequestSchema {
    #[schemars(
        description = "Use \"generate\" for a new image. Use \"edit\" only when source_images are provided. Defaults to \"generate\" when prompt is present."
    )]
    intent: Option<String>,
    #[schemars(description = "Text prompt for a new image. Required for intent=\"generate\".")]
    prompt: Option<String>,
    #[schemars(description = "Edit instruction. Required for intent=\"edit\".")]
    instruction: Option<String>,
    #[schemars(
        description = "Optional source images for edits, using Meerkat image references from earlier assistant images or blobs."
    )]
    source_images: Option<Vec<ImageSourceRef>>,
    #[schemars(
        description = "Optional reference images for generation, using Meerkat image references from earlier assistant images or blobs."
    )]
    reference_images: Option<Vec<ImageSourceRef>>,
    #[schemars(
        description = "Optional size: \"auto\", \"1024x1024\", \"1024x1536\", \"1536x1024\", or \"WIDTHxHEIGHT\"."
    )]
    size: Option<String>,
    #[schemars(description = "Optional quality: \"auto\", \"low\", \"medium\", or \"high\".")]
    quality: Option<String>,
    #[schemars(
        description = "Optional output format: \"auto\", \"png\", \"jpeg\", \"jpg\", or \"webp\"."
    )]
    format: Option<String>,
    #[schemars(description = "Optional number of images to generate. Defaults to 1.")]
    count: Option<u32>,
    #[schemars(description = "Optional provider override such as \"openai\" or \"gemini\".")]
    provider: Option<String>,
    #[schemars(description = "Optional model override for the selected provider.")]
    model: Option<String>,
    #[schemars(
        description = "Provider-specific image model parameters validated by the selected provider."
    )]
    provider_params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct SimpleGenerateImageToolRequest {
    #[serde(default)]
    intent: Option<Value>,
    #[serde(default)]
    prompt: Option<Value>,
    #[serde(default)]
    instruction: Option<Value>,
    #[serde(default)]
    source_images: Vec<ImageSourceRef>,
    #[serde(default)]
    reference_images: Vec<ImageSourceRef>,
    #[serde(default)]
    size: Option<Value>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    count: Option<NonZeroU32>,
    #[serde(default, alias = "n")]
    n: Option<NonZeroU32>,
    #[serde(default)]
    target: Option<Value>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider_params: Option<Value>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for GenerateImageTool {
    fn name(&self) -> &'static str {
        "generate_image"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: self.description(),
            input_schema: crate::schema::schema_for::<GenerateImageToolArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: GenerateImageToolArgs = serde_json::from_value(args)
            .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?;
        let operation_id = ImageOperationId::new(uuid::Uuid::new_v4());
        let request =
            parse_generate_image_request(args.request, operation_id, &*self.runtime.planner)?;

        let status = self
            .runtime
            .machine
            .session_model_routing_status(&self.runtime.session_id)
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        let resolved_plan = match self.runtime.planner.resolve_image_generation_plan(
            &status,
            operation_id,
            &request,
        ) {
            Ok(plan) => plan,
            Err(reason) => {
                return json_result(ImageGenerationToolResult {
                    operation_id,
                    terminal: ImageOperationTerminalClass::Denied { reason },
                    images: Vec::new(),
                    provider_text: ProviderTextDisposition::NotEmitted,
                    revised_prompt:
                        meerkat_core::image_generation::RevisedPromptDisposition::NotRequested,
                    native_metadata:
                        meerkat_core::image_generation::ProviderImageMetadata::NotEmitted,
                    warnings: Vec::new(),
                });
            }
        };

        let requires_scoped_override = execution_plan_requires_scoped_override(&resolved_plan);
        let (approval, approval_reason) = approval_for_resolved_plan(&resolved_plan);

        match self
            .runtime
            .machine
            .begin_image_operation(
                &self.runtime.session_id,
                ImageOperationRoutingRequest {
                    operation_id,
                    target_model: resolved_plan.machine_routing_model.clone(),
                    target_realtime: ModelRoutingRealtimePolicy {
                        target_realtime_capable: resolved_plan.machine_routing_realtime_capable,
                        allow_realtime_detach: false,
                    },
                    approval,
                    approval_reason,
                    requires_scoped_override,
                },
            )
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?
        {
            ImageOperationRoutingResult::Denied { reason, .. } => {
                return json_result(ImageGenerationToolResult {
                    operation_id,
                    terminal: ImageOperationTerminalClass::Denied { reason },
                    images: Vec::new(),
                    provider_text: ProviderTextDisposition::NotEmitted,
                    revised_prompt:
                        meerkat_core::image_generation::RevisedPromptDisposition::NotRequested,
                    native_metadata:
                        meerkat_core::image_generation::ProviderImageMetadata::NotEmitted,
                    warnings: Vec::new(),
                });
            }
            ImageOperationRoutingResult::Accepted { .. } => {}
        }

        if requires_scoped_override {
            self.runtime
                .machine
                .activate_image_operation_override(&self.runtime.session_id, operation_id)
                .await
                .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        }

        let provider_output = self
            .runtime
            .executor
            .execute_image_generation(ProviderImageGenerationRequest {
                operation_id,
                model: resolved_plan.provider_model.to_string(),
                generate_request: request,
                execution_plan: resolved_plan.execution_plan,
                projected_messages: resolved_plan.projected_messages,
            })
            .await;

        let (provider_output, committed_images, terminal) = match provider_output {
            Ok(output) => {
                let commit = commit_images(&*self.runtime.blob_store, &output).await;
                match commit {
                    Ok(images) => {
                        let terminal = output.terminal.clone();
                        (output, images, terminal)
                    }
                    Err(err) => {
                        warn!(
                            ?operation_id,
                            error = %err,
                            "failed to commit generated image blobs"
                        );
                        let mut failed = failed_provider_output(operation_id);
                        failed.warnings.push(
                            meerkat_core::ImageGenerationWarning::BlobCommitFailed { message: err },
                        );
                        (failed, Vec::new(), ImageOperationTerminalClass::Failed)
                    }
                }
            }
            Err(err) => {
                warn!(
                    ?operation_id,
                    error = %err,
                    "image generation provider execution failed"
                );
                let mut failed = failed_provider_output(operation_id);
                failed.warnings.push(
                    meerkat_core::ImageGenerationWarning::ProviderExecutionFailed {
                        message: err.to_string(),
                    },
                );
                (failed, Vec::new(), ImageOperationTerminalClass::Failed)
            }
        };

        let completed = match self
            .runtime
            .machine
            .complete_image_operation(&self.runtime.session_id, operation_id, terminal)
            .await
        {
            Ok(phase) => phase,
            Err(err) => {
                if requires_scoped_override
                    && let Err(restore_err) = self
                        .runtime
                        .machine
                        .restore_image_operation_override(&self.runtime.session_id, operation_id)
                        .await
                {
                    warn!(
                        ?operation_id,
                        error = %restore_err,
                        "failed to restore image operation override after completion error"
                    );
                }
                return Err(BuiltinToolError::execution_failed(err.to_string()));
            }
        };

        let terminal = if requires_scoped_override {
            let restored = self
                .runtime
                .machine
                .restore_image_operation_override(&self.runtime.session_id, operation_id)
                .await
                .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
            match restored {
                ImageOperationPhase::Terminal { terminal } => terminal,
                phase => {
                    return Err(BuiltinToolError::execution_failed(format!(
                        "image operation machine did not terminalize after scoped restore: {phase:?}"
                    )));
                }
            }
        } else {
            match completed {
                ImageOperationPhase::Terminal { terminal } => terminal,
                phase => {
                    return Err(BuiltinToolError::execution_failed(format!(
                        "image operation machine did not terminalize without scoped override: {phase:?}"
                    )));
                }
            }
        };

        let (provider_text, provider_text_warning) = capture_provider_text(
            &*self.runtime.blob_store,
            provider_output.provider_text.as_deref(),
        )
        .await;
        let mut result = provider_output.to_tool_result(committed_images.clone(), provider_text);
        if let Some(warning) = provider_text_warning {
            result.warnings.push(warning);
        }
        let result = ImageGenerationToolResult { terminal, ..result };
        let blocks = committed_images
            .into_iter()
            .map(|image| AssistantBlock::Image {
                image_id: image.image_id,
                blob_ref: image.blob_ref,
                media_type: image.media_type,
                width: image.width,
                height: image.height,
                revised_prompt: provider_output.revised_prompt.clone(),
                meta: provider_output.native_metadata.clone(),
            })
            .collect::<Vec<_>>();

        let value = serde_json::to_value(result)
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?;
        Ok(ToolOutput::JsonWithEffects {
            value,
            session_effects: if blocks.is_empty() {
                Vec::new()
            } else {
                vec![SessionEffect::AppendAssistantBlocks { blocks }]
            },
        })
    }
}

fn parse_generate_image_request(
    request: Value,
    operation_id: ImageOperationId,
    planner: &dyn ImageGenerationPlanner,
) -> Result<GenerateImageRequest, BuiltinToolError> {
    if let Ok(canonical) = serde_json::from_value::<GenerateImageRequest>(request.clone()) {
        return Ok(canonical);
    }

    let simple: SimpleGenerateImageToolRequest = serde_json::from_value(request)
        .map_err(|err| BuiltinToolError::invalid_args(format!("invalid image request: {err}")))?;

    let intent_kind = parse_simple_intent_kind(simple.intent.as_ref(), &simple)?;
    let source = PromptSource::ModelDistilled {
        tool_call_id: ToolCallId::new(format!("generate_image:{}", operation_id.0)),
    };
    let intent = match intent_kind {
        SimpleImageIntentKind::Generate => {
            let prompt = extract_text_field(
                "prompt",
                simple.prompt.as_ref().or_else(|| {
                    simple
                        .intent
                        .as_ref()
                        .and_then(|intent| intent.get("prompt"))
                }),
            )?;
            ImageGenerationIntent::Generate {
                prompt: PromptText::new(prompt)
                    .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?,
                prompt_source: source,
                reference_images: simple.reference_images,
            }
        }
        SimpleImageIntentKind::Edit => {
            let instruction = extract_text_field(
                "instruction",
                simple.instruction.as_ref().or_else(|| {
                    simple
                        .intent
                        .as_ref()
                        .and_then(|intent| intent.get("instruction"))
                }),
            )?;
            ImageGenerationIntent::Edit {
                instruction: PromptText::new(instruction)
                    .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))?,
                instruction_source: source,
                source_images: simple.source_images,
            }
        }
    };

    GenerateImageRequest::with_provider_params(
        intent,
        parse_simple_target(
            simple.target.as_ref(),
            simple.provider.as_deref(),
            simple.model.as_deref(),
            planner,
        )?,
        parse_simple_size(simple.size.as_ref())?,
        parse_simple_quality(simple.quality.as_deref())?,
        parse_simple_format(simple.format.as_deref())?,
        simple.count.or(simple.n).unwrap_or(NonZeroU32::MIN),
        simple.provider_params,
    )
    .map_err(|err| BuiltinToolError::invalid_args(err.to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimpleImageIntentKind {
    Generate,
    Edit,
}

fn parse_simple_intent_kind(
    intent: Option<&Value>,
    request: &SimpleGenerateImageToolRequest,
) -> Result<SimpleImageIntentKind, BuiltinToolError> {
    let Some(intent) = intent else {
        return if request.instruction.is_some() || !request.source_images.is_empty() {
            Ok(SimpleImageIntentKind::Edit)
        } else {
            Ok(SimpleImageIntentKind::Generate)
        };
    };
    match intent {
        Value::String(value) => parse_intent_name(value),
        Value::Object(map) => map
            .get("intent")
            .or_else(|| map.get("type"))
            .and_then(Value::as_str)
            .map(parse_intent_name)
            .unwrap_or_else(|| {
                Err(BuiltinToolError::invalid_args(
                    "request.intent object must include intent=\"generate\" or intent=\"edit\"",
                ))
            }),
        _ => Err(BuiltinToolError::invalid_args(
            "request.intent must be \"generate\", \"edit\", or an object with an intent field",
        )),
    }
}

fn parse_intent_name(value: &str) -> Result<SimpleImageIntentKind, BuiltinToolError> {
    match normalize_choice(value).as_str() {
        "generate" | "create" | "new" => Ok(SimpleImageIntentKind::Generate),
        "edit" | "modify" => Ok(SimpleImageIntentKind::Edit),
        other => Err(BuiltinToolError::invalid_args(format!(
            "unsupported image intent '{other}'; use 'generate' or 'edit'"
        ))),
    }
}

fn extract_text_field(
    field: &'static str,
    value: Option<&Value>,
) -> Result<String, BuiltinToolError> {
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Object(map)) => map
            .get("content")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                BuiltinToolError::invalid_args(format!(
                    "request.{field} object must include a string content field"
                ))
            }),
        Some(_) => Err(BuiltinToolError::invalid_args(format!(
            "request.{field} must be a string"
        ))),
        None => Err(BuiltinToolError::invalid_args(format!(
            "request.{field} is required"
        ))),
    }
}

fn parse_simple_target(
    target: Option<&Value>,
    provider: Option<&str>,
    model: Option<&str>,
    planner: &dyn ImageGenerationPlanner,
) -> Result<ImageGenerationTargetPreference, BuiltinToolError> {
    if let Some(target) = target {
        if let Ok(canonical) =
            serde_json::from_value::<ImageGenerationTargetPreference>(target.clone())
        {
            return Ok(canonical);
        }
        if let Some(value) = target.as_str() {
            if normalize_choice(value) == "auto" {
                return Ok(ImageGenerationTargetPreference::Auto);
            }
            return Ok(ImageGenerationTargetPreference::ProviderDefault {
                provider: ProviderId::new(value),
            });
        }
    }

    match (provider, model) {
        (Some(provider), Some(model)) => Ok(ImageGenerationTargetPreference::Model {
            provider: ProviderId::new(provider),
            model: ModelId::new(model),
        }),
        (Some(provider), None) => Ok(ImageGenerationTargetPreference::ProviderDefault {
            provider: ProviderId::new(provider),
        }),
        (None, Some(model)) => {
            let provider = planner.infer_provider_for_model(model).ok_or_else(|| {
                BuiltinToolError::invalid_args(
                    "request.model requires request.provider unless the configured image providers own that model",
                )
            })?;
            Ok(ImageGenerationTargetPreference::Model {
                provider,
                model: ModelId::new(model),
            })
        }
        (None, None) => Ok(ImageGenerationTargetPreference::Auto),
    }
}

fn parse_simple_size(size: Option<&Value>) -> Result<ImageSizePreference, BuiltinToolError> {
    let Some(size) = size else {
        return Ok(ImageSizePreference::Auto);
    };
    if let Ok(canonical) = serde_json::from_value::<ImageSizePreference>(size.clone()) {
        return Ok(canonical);
    }
    let value = size.as_str().ok_or_else(|| {
        BuiltinToolError::invalid_args("request.size must be a string or canonical size object")
    })?;
    let normalized = normalize_choice(value);
    match normalized.as_str() {
        "auto" => Ok(ImageSizePreference::Auto),
        "square" | "square1024" | "1024x1024" => Ok(ImageSizePreference::Square1024),
        "portrait" | "portrait1024x1536" | "1024x1536" => {
            Ok(ImageSizePreference::Portrait1024x1536)
        }
        "landscape" | "landscape1536x1024" | "1536x1024" => {
            Ok(ImageSizePreference::Landscape1536x1024)
        }
        custom => parse_custom_size(custom),
    }
}

fn parse_custom_size(value: &str) -> Result<ImageSizePreference, BuiltinToolError> {
    let Some((width, height)) = value.split_once('x') else {
        return Err(BuiltinToolError::invalid_args(format!(
            "unsupported image size '{value}'; use auto, 1024x1024, 1024x1536, 1536x1024, or WIDTHxHEIGHT"
        )));
    };
    let width = width
        .parse::<u32>()
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| BuiltinToolError::invalid_args("custom image width must be non-zero"))?;
    let height = height
        .parse::<u32>()
        .ok()
        .and_then(NonZeroU32::new)
        .ok_or_else(|| BuiltinToolError::invalid_args("custom image height must be non-zero"))?;
    Ok(ImageSizePreference::Custom { width, height })
}

fn parse_simple_quality(quality: Option<&str>) -> Result<ImageQualityPreference, BuiltinToolError> {
    match quality.map(normalize_choice).as_deref() {
        None | Some("auto") => Ok(ImageQualityPreference::Auto),
        Some("low") => Ok(ImageQualityPreference::Low),
        Some("medium") => Ok(ImageQualityPreference::Medium),
        Some("high") => Ok(ImageQualityPreference::High),
        Some(other) => Err(BuiltinToolError::invalid_args(format!(
            "unsupported image quality '{other}'; use auto, low, medium, or high"
        ))),
    }
}

fn parse_simple_format(format: Option<&str>) -> Result<ImageFormatPreference, BuiltinToolError> {
    match format.map(normalize_choice).as_deref() {
        None | Some("auto") => Ok(ImageFormatPreference::Auto),
        Some("png") => Ok(ImageFormatPreference::Png),
        Some("jpeg" | "jpg") => Ok(ImageFormatPreference::Jpeg),
        Some("webp") => Ok(ImageFormatPreference::Webp),
        Some(other) => Err(BuiltinToolError::invalid_args(format!(
            "unsupported image format '{other}'; use auto, png, jpeg, jpg, or webp"
        ))),
    }
}

fn normalize_choice(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-', ' '], "")
}

async fn commit_images(
    blob_store: &dyn BlobStore,
    output: &ProviderImageGenerationOutput,
) -> Result<Vec<AssistantImageRef>, String> {
    if !matches!(output.terminal, ImageOperationTerminalClass::Generated) {
        return Ok(Vec::new());
    }
    let mut committed = Vec::with_capacity(output.images.len());
    for image in &output.images {
        let blob_ref = blob_store
            .put_image(image.media_type.as_str(), &image.base64_data)
            .await
            .map_err(|err| err.to_string())?;
        committed.push(AssistantImageRef {
            image_id: AssistantImageId::new(uuid::Uuid::new_v4()),
            blob_ref,
            media_type: image.media_type.clone(),
            width: image.width,
            height: image.height,
        });
    }
    Ok(committed)
}

fn execution_plan_requires_scoped_override(plan: &ImageGenerationResolvedPlan) -> bool {
    plan.execution_plan.requires_scoped_override()
}

fn approval_for_resolved_plan(
    _plan: &ImageGenerationResolvedPlan,
) -> (
    ModelRoutingApprovalDisposition,
    Option<meerkat_core::image_generation::ImageOperationApprovalReason>,
) {
    (ModelRoutingApprovalDisposition::NotRequired, None)
}

async fn capture_provider_text(
    blob_store: &dyn BlobStore,
    provider_text: Option<&str>,
) -> (
    ProviderTextDisposition,
    Option<meerkat_core::ImageGenerationWarning>,
) {
    let Some(provider_text) = provider_text.filter(|text| !text.is_empty()) else {
        return (ProviderTextDisposition::NotEmitted, None);
    };
    let data = base64::engine::general_purpose::STANDARD.encode(provider_text.as_bytes());
    match blob_store
        .put_image("text/plain; charset=utf-8", &data)
        .await
    {
        Ok(blob_ref) => (
            ProviderTextDisposition::Captured {
                text_artifact_ref: TextArtifactRef::new(blob_ref.blob_id.to_string()),
            },
            None,
        ),
        Err(err) => {
            warn!(
                error = %err,
                "failed to capture provider image text artifact"
            );
            (
                ProviderTextDisposition::EmittedButNotStored,
                Some(
                    meerkat_core::ImageGenerationWarning::ProviderTextCaptureFailed {
                        message: err.to_string(),
                    },
                ),
            )
        }
    }
}

fn failed_provider_output(operation_id: ImageOperationId) -> ProviderImageGenerationOutput {
    ProviderImageGenerationOutput {
        operation_id,
        terminal: ImageOperationTerminalClass::Failed,
        images: Vec::new(),
        provider_text: None,
        revised_prompt: meerkat_core::image_generation::RevisedPromptDisposition::NotRequested,
        native_metadata: meerkat_core::image_generation::ProviderImageMetadata::NotEmitted,
        warnings: Vec::new(),
    }
}

#[allow(dead_code)]
fn post_activation_terminal(terminal: &ImageOperationTerminalClass) -> PostActivationImageTerminal {
    match terminal {
        ImageOperationTerminalClass::Generated => PostActivationImageTerminal::Generated,
        ImageOperationTerminalClass::EmptyResult { .. } => PostActivationImageTerminal::EmptyResult,
        ImageOperationTerminalClass::Denied { reason } => match reason {
            ImageOperationDenialReason::CostPolicy => PostActivationImageTerminal::Denied {
                reason: PostActivationImageDenialReason::CostPolicy,
            },
            ImageOperationDenialReason::SafetyPolicy => PostActivationImageTerminal::Denied {
                reason: PostActivationImageDenialReason::SafetyPolicy,
            },
            ImageOperationDenialReason::DeniedDuringApproval { approvable } => {
                PostActivationImageTerminal::Denied {
                    reason: PostActivationImageDenialReason::DeniedDuringApproval {
                        approvable: *approvable,
                    },
                }
            }
            ImageOperationDenialReason::ScopedOverrideConflict => {
                PostActivationImageTerminal::Denied {
                    reason: PostActivationImageDenialReason::ScopedOverrideConflict,
                }
            }
            ImageOperationDenialReason::RealtimeTransportConflict => {
                PostActivationImageTerminal::Denied {
                    reason: PostActivationImageDenialReason::RealtimeTransportConflict,
                }
            }
            ImageOperationDenialReason::ProjectionUnsupported => {
                PostActivationImageTerminal::Denied {
                    reason: PostActivationImageDenialReason::ProjectionUnsupported,
                }
            }
            ImageOperationDenialReason::UnsupportedTarget
            | ImageOperationDenialReason::UnsupportedCount
            | ImageOperationDenialReason::CapabilityPolicy
            | ImageOperationDenialReason::ApprovalRequiredButUnavailable => {
                PostActivationImageTerminal::Failed
            }
        },
        ImageOperationTerminalClass::RefusedByProvider => {
            PostActivationImageTerminal::RefusedByProvider
        }
        ImageOperationTerminalClass::SafetyFiltered => PostActivationImageTerminal::SafetyFiltered,
        ImageOperationTerminalClass::Failed
        | ImageOperationTerminalClass::ScopedRestoreFailed { .. } => {
            PostActivationImageTerminal::Failed
        }
        ImageOperationTerminalClass::Cancelled => PostActivationImageTerminal::Cancelled,
        ImageOperationTerminalClass::Timeout => PostActivationImageTerminal::Timeout,
    }
}

fn json_result(result: ImageGenerationToolResult) -> Result<ToolOutput, BuiltinToolError> {
    serde_json::to_value(result)
        .map(ToolOutput::Json)
        .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::image_generation::{
        ImageContinuityTokenSupport, ImageFormatPreference, ImageGenerationBackendKind,
        ImageGenerationIntent, ImageGenerationTargetCapabilities, ImageGenerationTargetPreference,
        ImageQualityPreference, ImageSizePreference, PromptSource, PromptText, ProviderId,
        RevisedPromptDisposition, ToolCallId,
    };
    use meerkat_core::lifecycle::run_primitive::ModelId;
    use meerkat_core::types::{ContentBlock, ToolCallView};
    use meerkat_core::{AgentToolDispatcher, BlobId, BlobPayload, BlobRef, BlobStoreError};
    use meerkat_llm_core::ProviderGeneratedImage;
    use serde_json::json;
    use serde_json::value::RawValue;
    use std::num::NonZeroU32;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeMachine {
        completed: Mutex<Option<ImageOperationTerminalClass>>,
        calls: Mutex<Vec<&'static str>>,
        requires_scoped_override: Mutex<bool>,
    }

    struct FakePlanner;

    impl ImageGenerationPlanner for FakePlanner {
        fn resolve_image_generation_plan(
            &self,
            status: &meerkat_core::image_generation::SessionModelRoutingStatus,
            _operation_id: ImageOperationId,
            request: &GenerateImageRequest,
        ) -> Result<ImageGenerationResolvedPlan, ImageOperationDenialReason> {
            if request.count > NonZeroU32::MIN {
                return Err(ImageOperationDenialReason::UnsupportedCount);
            }
            let provider = match &request.target {
                ImageGenerationTargetPreference::Model { provider, .. }
                | ImageGenerationTargetPreference::ProviderDefault { provider } => {
                    provider.0.clone()
                }
                ImageGenerationTargetPreference::Auto => "openai".to_string(),
            };
            let requires_scoped_override = provider == "gemini" || provider == "google";
            Ok(ImageGenerationResolvedPlan {
                provider_model: if requires_scoped_override {
                    ModelId::new("native-image-model")
                } else {
                    ModelId::new("hosted-image-model")
                },
                machine_routing_model: status.effective_model.clone(),
                machine_routing_realtime_capable: true,
                execution_plan: meerkat_core::GenerateImageExecutionPlan {
                    provider: ProviderId::new(provider),
                    backend: if requires_scoped_override {
                        ImageGenerationBackendKind::NativeModel
                    } else {
                        ImageGenerationBackendKind::HostedTool
                    },
                    max_count: NonZeroU32::MIN,
                    capabilities: ImageGenerationTargetCapabilities {
                        hosted_image_generation_tool: true,
                        native_image_output: true,
                        custom_tools: false,
                        image_search_grounding: false,
                        image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
                    },
                    requires_scoped_override,
                    provider_plan: serde_json::Value::Null,
                },
                projected_messages: Vec::new(),
            })
        }

        fn infer_provider_for_model(&self, model: &str) -> Option<ProviderId> {
            if model.starts_with("owned-openai") {
                Some(ProviderId::new("openai"))
            } else if model.starts_with("owned-gemini") {
                Some(ProviderId::new("gemini"))
            } else {
                None
            }
        }

        fn provider_documentation(&self) -> Vec<String> {
            vec!["FakeProvider:\n- provider_params: {\"fake\":true}.".to_string()]
        }
    }

    fn fake_planner() -> Arc<dyn ImageGenerationPlanner> {
        Arc::new(FakePlanner)
    }

    #[async_trait]
    impl ImageGenerationMachine for FakeMachine {
        async fn session_model_routing_status(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError>
        {
            Ok(
                meerkat_core::image_generation::SessionModelRoutingStatus::new(
                    ModelId::new("hosted-session-model"),
                    None,
                    None,
                    None,
                ),
            )
        }

        async fn begin_image_operation(
            &self,
            _session_id: &SessionId,
            request: ImageOperationRoutingRequest,
        ) -> Result<ImageOperationRoutingResult, RuntimeDriverError> {
            self.calls.lock().unwrap().push("begin");
            *self.requires_scoped_override.lock().unwrap() = request.requires_scoped_override;
            Ok(ImageOperationRoutingResult::Accepted {
                operation_id: request.operation_id,
                phase: ImageOperationPhase::PlanResolved,
            })
        }

        async fn activate_image_operation_override(
            &self,
            _session_id: &SessionId,
            _operation_id: ImageOperationId,
        ) -> Result<ImageOperationPhase, RuntimeDriverError> {
            self.calls.lock().unwrap().push("activate");
            Ok(ImageOperationPhase::ScopedOverrideActive)
        }

        async fn complete_image_operation(
            &self,
            _session_id: &SessionId,
            _operation_id: ImageOperationId,
            terminal: ImageOperationTerminalClass,
        ) -> Result<ImageOperationPhase, RuntimeDriverError> {
            self.calls.lock().unwrap().push("complete");
            *self.completed.lock().unwrap() = Some(terminal);
            if *self.requires_scoped_override.lock().unwrap() {
                Ok(ImageOperationPhase::RestoringScopedOverride)
            } else {
                Ok(ImageOperationPhase::Terminal {
                    terminal: self
                        .completed
                        .lock()
                        .unwrap()
                        .clone()
                        .unwrap_or(ImageOperationTerminalClass::Failed),
                })
            }
        }

        async fn restore_image_operation_override(
            &self,
            _session_id: &SessionId,
            _operation_id: ImageOperationId,
        ) -> Result<ImageOperationPhase, RuntimeDriverError> {
            self.calls.lock().unwrap().push("restore");
            Ok(ImageOperationPhase::Terminal {
                terminal: self
                    .completed
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or(ImageOperationTerminalClass::Failed),
            })
        }
    }

    struct FakeBlobStore {
        writes: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl BlobStore for FakeBlobStore {
        async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
            self.writes
                .lock()
                .unwrap()
                .push((media_type.to_string(), data.to_string()));
            Ok(BlobRef {
                blob_id: BlobId::new("blob-generated"),
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            Err(BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, _blob_id: &BlobId) -> Result<(), BlobStoreError> {
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            true
        }
    }

    struct FakeExecutor;

    #[async_trait]
    impl ImageGenerationExecutor for FakeExecutor {
        async fn execute_image_generation(
            &self,
            request: ProviderImageGenerationRequest,
        ) -> Result<ProviderImageGenerationOutput, meerkat_llm_core::LlmError> {
            assert!(matches!(
                request.model.as_str(),
                "hosted-image-model" | "native-image-model"
            ));
            Ok(ProviderImageGenerationOutput {
                operation_id: request.operation_id,
                terminal: ImageOperationTerminalClass::Generated,
                images: vec![ProviderGeneratedImage {
                    media_type: meerkat_core::MediaType::new("image/png"),
                    base64_data: "iVBORw0KGgo=".to_string(),
                    width: 1,
                    height: 1,
                }],
                provider_text: Some("caption".to_string()),
                revised_prompt: RevisedPromptDisposition::NotRequested,
                native_metadata: meerkat_core::image_generation::ProviderImageMetadata::NotEmitted,
                warnings: Vec::new(),
            })
        }
    }

    fn request() -> GenerateImageRequest {
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
            ImageQualityPreference::Auto,
            ImageFormatPreference::Png,
            NonZeroU32::new(1).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn generate_image_schema_documents_simple_model_facing_request() {
        let schema = crate::schema::schema_for::<GenerateImageToolArgs>();
        let request = schema
            .pointer("/properties/request")
            .expect("request schema should be present");
        assert!(
            request
                .get("description")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|description| {
                    description.contains("\"intent\":\"generate\"")
                        && description.contains("\"size\":\"1024x1024\"")
                }),
            "request description should include a model-usable example: {request:#?}"
        );
        let request = schema
            .pointer("/$defs/GenerateImageToolRequestSchema")
            .expect("request schema definition should be present");
        assert!(
            request.pointer("/properties/prompt").is_some(),
            "request schema should expose prompt directly: {request:#?}"
        );
        assert!(
            request.pointer("/properties/size").is_some(),
            "request schema should expose size directly: {request:#?}"
        );
    }

    #[test]
    fn generate_image_description_documents_model_routing_defaults() {
        let runtime = ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: Arc::new(FakeMachine::default()),
            planner: fake_planner(),
            blob_store: Arc::new(FakeBlobStore {
                writes: Mutex::new(Vec::new()),
            }),
            executor: Arc::new(FakeExecutor),
        };
        let description = GenerateImageTool::new(runtime).def().description;

        for expected in [
            "registered image default",
            "catalog identifies a configured provider for that model",
            "count/n: currently only 1 is supported",
            "provider_params",
            "Size support is model/provider dependent",
        ] {
            assert!(
                description.contains(expected),
                "tool description should document {expected:?}: {description}"
            );
        }
    }

    #[test]
    fn cross_provider_image_plan_does_not_require_unavailable_approval() {
        let plan = ImageGenerationResolvedPlan {
            provider_model: ModelId::new("gemini-3.1-flash-image-preview"),
            machine_routing_model: ModelId::new("gpt-5.5"),
            machine_routing_realtime_capable: true,
            execution_plan: meerkat_core::GenerateImageExecutionPlan {
                provider: ProviderId::new("gemini"),
                backend: ImageGenerationBackendKind::NativeModel,
                max_count: NonZeroU32::MIN,
                capabilities: ImageGenerationTargetCapabilities {
                    hosted_image_generation_tool: true,
                    native_image_output: true,
                    custom_tools: false,
                    image_search_grounding: false,
                    image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
                },
                requires_scoped_override: true,
                provider_plan: serde_json::Value::Null,
            },
            projected_messages: Vec::new(),
        };

        let (approval, approval_reason) = approval_for_resolved_plan(&plan);

        assert_eq!(approval, ModelRoutingApprovalDisposition::NotRequired);
        assert!(approval_reason.is_none());
    }

    #[test]
    fn generate_image_accepts_simple_generate_request() {
        let parsed = parse_generate_image_request(
            json!({
                "intent": "generate",
                "prompt": "draw a cozy tabby cat",
                "size": "1024x1024",
                "quality": "high",
                "format": "png",
                "n": 1
            }),
            ImageOperationId::new(uuid::Uuid::new_v4()),
            &FakePlanner,
        )
        .unwrap();

        match parsed.intent {
            ImageGenerationIntent::Generate { prompt, .. } => {
                assert_eq!(prompt.content, "draw a cozy tabby cat");
            }
            other => panic!("expected generate intent, got {other:?}"),
        }
        assert_eq!(parsed.size, ImageSizePreference::Square1024);
        assert_eq!(parsed.quality, ImageQualityPreference::High);
        assert_eq!(parsed.format, ImageFormatPreference::Png);
        assert_eq!(parsed.count, NonZeroU32::new(1).unwrap());
    }

    #[test]
    fn generate_image_accepts_model_only_for_known_image_models() {
        let parsed = parse_generate_image_request(
            json!({
                "intent": "generate",
                "prompt": "draw a cozy tabby cat",
                "model": "owned-openai-image-model"
            }),
            ImageOperationId::new(uuid::Uuid::new_v4()),
            &FakePlanner,
        )
        .unwrap();

        assert!(matches!(
            parsed.target,
            ImageGenerationTargetPreference::Model { ref provider, ref model }
                if provider.0 == "openai" && model.as_str() == "owned-openai-image-model"
        ));

        let parsed = parse_generate_image_request(
            json!({
                "intent": "generate",
                "prompt": "draw a cozy tabby cat",
                "model": "owned-gemini-image-model"
            }),
            ImageOperationId::new(uuid::Uuid::new_v4()),
            &FakePlanner,
        )
        .unwrap();

        assert!(matches!(
            parsed.target,
            ImageGenerationTargetPreference::Model { ref provider, ref model }
                if provider.0 == "gemini" && model.as_str() == "owned-gemini-image-model"
        ));
    }

    #[test]
    fn generate_image_defaults_intent_for_prompt_only_request() {
        let parsed = parse_generate_image_request(
            json!({
                "prompt": "draw a cozy tabby cat"
            }),
            ImageOperationId::new(uuid::Uuid::new_v4()),
            &FakePlanner,
        )
        .unwrap();

        match parsed.intent {
            ImageGenerationIntent::Generate { prompt, .. } => {
                assert_eq!(prompt.content, "draw a cozy tabby cat");
            }
            other => panic!("expected generate intent, got {other:?}"),
        }
        assert_eq!(parsed.size, ImageSizePreference::Auto);
        assert_eq!(parsed.quality, ImageQualityPreference::Auto);
        assert_eq!(parsed.format, ImageFormatPreference::Auto);
        assert_eq!(parsed.count, NonZeroU32::new(1).unwrap());
    }

    #[test]
    fn generate_image_accepts_create_intent_object() {
        let parsed = parse_generate_image_request(
            json!({
                "intent": { "type": "create" },
                "prompt": "draw a cozy tabby cat"
            }),
            ImageOperationId::new(uuid::Uuid::new_v4()),
            &FakePlanner,
        )
        .unwrap();

        assert!(matches!(
            parsed.intent,
            ImageGenerationIntent::Generate { .. }
        ));
    }

    #[tokio::test]
    async fn generate_image_dispatch_commits_blob_and_emits_assistant_image_effect() {
        let machine = Arc::new(FakeMachine::default());
        let blob_store = Arc::new(FakeBlobStore {
            writes: Mutex::new(Vec::new()),
        });
        let runtime = ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: machine.clone(),
            planner: fake_planner(),
            blob_store: blob_store.clone(),
            executor: Arc::new(FakeExecutor),
        };
        let mut dispatcher = crate::builtin::CompositeDispatcher::new(
            Arc::new(crate::builtin::MemoryTaskStore::new()),
            &crate::builtin::BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        dispatcher
            .register_image_generation_tool(runtime, meerkat_core::ToolCategoryOverride::Enable);

        let raw = RawValue::from_string(
            serde_json::to_string(&json!({
                "request": request()
            }))
            .unwrap(),
        )
        .unwrap();
        let outcome = dispatcher
            .dispatch(ToolCallView {
                id: "call-1",
                name: "generate_image",
                args: &raw,
            })
            .await
            .unwrap();

        assert_eq!(
            machine.calls.lock().unwrap().as_slice(),
            ["begin", "complete"]
        );
        assert_eq!(
            blob_store.writes.lock().unwrap().as_slice(),
            [
                ("image/png".to_string(), "iVBORw0KGgo=".to_string()),
                (
                    "text/plain; charset=utf-8".to_string(),
                    "Y2FwdGlvbg==".to_string()
                )
            ]
        );
        assert_eq!(outcome.session_effects.len(), 1);
        let SessionEffect::AppendAssistantBlocks { blocks } = &outcome.session_effects[0] else {
            panic!("expected assistant image blocks effect");
        };
        assert!(matches!(blocks.as_slice(), [AssistantBlock::Image { .. }]));
        let ContentBlock::Text { text } = &outcome.result.content[0] else {
            panic!("expected JSON text tool result");
        };
        let result: ImageGenerationToolResult = serde_json::from_str(text).unwrap();
        assert!(matches!(
            result.terminal,
            ImageOperationTerminalClass::Generated
        ));
        assert_eq!(result.images.len(), 1);
        assert!(matches!(
            result.provider_text,
            ProviderTextDisposition::Captured { .. }
        ));
    }

    #[tokio::test]
    async fn generate_image_survives_ops_lifecycle_rebind() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let machine = Arc::new(FakeMachine::default());
        let runtime = ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine,
            planner: fake_planner(),
            blob_store: Arc::new(FakeBlobStore {
                writes: Mutex::new(Vec::new()),
            }),
            executor: Arc::new(FakeExecutor),
        };
        let mut dispatcher = crate::builtin::CompositeDispatcher::new(
            Arc::new(crate::builtin::MemoryTaskStore::new()),
            &crate::builtin::BuiltinToolConfig::default(),
            None,
            Some(crate::builtin::shell::ShellConfig::with_project_root(
                temp_dir.path().to_path_buf(),
            )),
            None,
            Some(SessionId::new().to_string()),
            true,
        )
        .unwrap();
        dispatcher
            .register_image_generation_tool(runtime, meerkat_core::ToolCategoryOverride::Enable);

        let registry: Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry> =
            Arc::new(meerkat_runtime::RuntimeOpsLifecycleRegistry::new());
        let rebound = Arc::new(dispatcher)
            .bind_ops_lifecycle(registry, SessionId::new())
            .unwrap()
            .into_dispatcher();

        assert!(
            rebound
                .tools()
                .iter()
                .any(|tool| tool.name == "generate_image"),
            "ops lifecycle rebinding must preserve late-registered image generation tool"
        );
    }

    #[test]
    fn generate_image_registration_respects_visibility_override() {
        let runtime = ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: Arc::new(FakeMachine::default()),
            planner: fake_planner(),
            blob_store: Arc::new(FakeBlobStore {
                writes: Mutex::new(Vec::new()),
            }),
            executor: Arc::new(FakeExecutor),
        };
        let mut dispatcher = crate::builtin::CompositeDispatcher::new(
            Arc::new(crate::builtin::MemoryTaskStore::new()),
            &crate::builtin::BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            true,
        )
        .unwrap();
        dispatcher
            .register_image_generation_tool(runtime, meerkat_core::ToolCategoryOverride::Disable);

        assert!(
            !dispatcher
                .tools()
                .iter()
                .any(|tool| tool.name == "generate_image"),
            "explicit image_generation disable must hide generate_image even when runtime exists"
        );
    }

    #[tokio::test]
    async fn generate_image_gemini_plan_uses_scoped_override_call_sequence() {
        let machine = Arc::new(FakeMachine::default());
        let tool = GenerateImageTool::new(ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: machine.clone(),
            planner: fake_planner(),
            blob_store: Arc::new(FakeBlobStore {
                writes: Mutex::new(Vec::new()),
            }),
            executor: Arc::new(FakeExecutor),
        });
        let mut image_request = request();
        image_request.target = ImageGenerationTargetPreference::ProviderDefault {
            provider: ProviderId::new("gemini"),
        };

        let output = tool
            .call(json!({
                "request": image_request
            }))
            .await
            .unwrap()
            .into_json()
            .unwrap();
        let result: ImageGenerationToolResult = serde_json::from_value(output).unwrap();

        assert!(matches!(
            result.terminal,
            ImageOperationTerminalClass::Generated
        ));
        assert_eq!(
            machine.calls.lock().unwrap().as_slice(),
            ["begin", "activate", "complete", "restore"]
        );
    }

    #[tokio::test]
    async fn generate_image_rejects_unsupported_count_during_machine_planning() {
        let machine = Arc::new(FakeMachine::default());
        let tool = GenerateImageTool::new(ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: machine.clone(),
            planner: fake_planner(),
            blob_store: Arc::new(FakeBlobStore {
                writes: Mutex::new(Vec::new()),
            }),
            executor: Arc::new(FakeExecutor),
        });
        let mut too_many = request();
        too_many.count = NonZeroU32::new(2).unwrap();
        let output = tool
            .call(json!({
                "request": too_many
            }))
            .await
            .unwrap()
            .into_json()
            .unwrap();
        let result: ImageGenerationToolResult = serde_json::from_value(output).unwrap();
        assert!(matches!(
            result.terminal,
            ImageOperationTerminalClass::Denied {
                reason: ImageOperationDenialReason::UnsupportedCount
            }
        ));
        assert!(machine.calls.lock().unwrap().is_empty());
    }
}
