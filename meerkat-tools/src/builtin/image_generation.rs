//! Builtin assistant image generation tool.

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use base64::Engine;
use meerkat_core::image_generation::{
    AssistantImageId, AssistantImageRef, GenerateImageExecutionPlan, GenerateImageRequest,
    ImageGenerationResolvedPlan, ImageGenerationToolResult, ImageOperationApprovalReason,
    ImageOperationDenialReason, ImageOperationId, ImageOperationPhase, ImageOperationTerminalClass,
    PostActivationImageDenialReason, PostActivationImageTerminal, ProviderTextDisposition,
    TextArtifactRef,
};
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
use std::sync::Arc;
use tracing::warn;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ImageGenerationMachine: Send + Sync {
    async fn session_model_routing_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError>;

    async fn resolve_image_generation_plan(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
        request: &GenerateImageRequest,
    ) -> Result<Result<ImageGenerationResolvedPlan, ImageOperationDenialReason>, RuntimeDriverError>;

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

    async fn resolve_image_generation_plan(
        &self,
        session_id: &SessionId,
        operation_id: ImageOperationId,
        request: &GenerateImageRequest,
    ) -> Result<Result<ImageGenerationResolvedPlan, ImageOperationDenialReason>, RuntimeDriverError>
    {
        SessionServiceRuntimeExt::resolve_image_generation_plan(
            self,
            session_id,
            operation_id,
            request,
        )
        .await
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
    pub blob_store: Arc<dyn BlobStore>,
    pub executor: Arc<dyn ImageGenerationExecutor>,
}

#[derive(Clone)]
pub struct GenerateImageTool {
    runtime: ImageGenerationToolRuntime,
}

impl GenerateImageTool {
    pub fn new(runtime: ImageGenerationToolRuntime) -> Self {
        Self { runtime }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct GenerateImageToolArgs {
    #[schemars(with = "serde_json::Value")]
    request: GenerateImageRequest,
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
            description:
                "Generate or edit an assistant image through the session-owned image substrate."
                    .into(),
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

        let resolved_plan = match self
            .runtime
            .machine
            .resolve_image_generation_plan(&self.runtime.session_id, operation_id, &args.request)
            .await
            .map_err(|err| BuiltinToolError::execution_failed(err.to_string()))?
        {
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
                generate_request: args.request,
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
    matches!(
        plan.execution_plan,
        GenerateImageExecutionPlan::GeminiNativeImageModel { .. }
    )
}

fn approval_for_resolved_plan(
    plan: &ImageGenerationResolvedPlan,
) -> (
    ModelRoutingApprovalDisposition,
    Option<ImageOperationApprovalReason>,
) {
    let session_provider =
        meerkat_core::Provider::infer_from_model(plan.machine_routing_model.as_str());
    let target_provider = meerkat_core::Provider::infer_from_model(plan.provider_model.as_str());

    if session_provider.is_some()
        && target_provider.is_some()
        && session_provider != target_provider
    {
        (
            ModelRoutingApprovalDisposition::RequiredButUnavailable,
            Some(ImageOperationApprovalReason::CrossProvider),
        )
    } else {
        (ModelRoutingApprovalDisposition::NotRequired, None)
    }
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
        ImageFormatPreference, ImageGenerationIntent, ImageGenerationTargetPreference,
        ImageQualityPreference, ImageSizePreference, PromptSource, PromptText,
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

    #[async_trait]
    impl ImageGenerationMachine for FakeMachine {
        async fn session_model_routing_status(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError>
        {
            Ok(
                meerkat_core::image_generation::SessionModelRoutingStatus::new(
                    ModelId::new("gpt-5.4"),
                    None,
                    None,
                    None,
                ),
            )
        }

        async fn resolve_image_generation_plan(
            &self,
            session_id: &SessionId,
            operation_id: ImageOperationId,
            request: &GenerateImageRequest,
        ) -> Result<
            Result<ImageGenerationResolvedPlan, ImageOperationDenialReason>,
            RuntimeDriverError,
        > {
            self.calls.lock().unwrap().push("resolve_plan");
            let status = self.session_model_routing_status(session_id).await?;
            Ok(meerkat_runtime::resolve_image_generation_plan_from_status(
                &status,
                operation_id,
                request,
            ))
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
            assert_eq!(request.model, "gpt-5.4");
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

    #[tokio::test]
    async fn generate_image_dispatch_commits_blob_and_emits_assistant_image_effect() {
        let machine = Arc::new(FakeMachine::default());
        let blob_store = Arc::new(FakeBlobStore {
            writes: Mutex::new(Vec::new()),
        });
        let runtime = ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: machine.clone(),
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
        dispatcher.register_image_generation_tool(runtime);

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
            ["resolve_plan", "begin", "complete"]
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
    async fn generate_image_rejects_unsupported_count_during_machine_planning() {
        let machine = Arc::new(FakeMachine::default());
        let tool = GenerateImageTool::new(ImageGenerationToolRuntime {
            session_id: SessionId::new(),
            machine: machine.clone(),
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
        assert_eq!(machine.calls.lock().unwrap().as_slice(), ["resolve_plan"]);
    }
}
