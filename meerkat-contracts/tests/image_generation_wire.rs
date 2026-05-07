#![allow(clippy::unwrap_used)]

use std::num::NonZeroU32;

use meerkat_contracts::wire::{
    WireAssistantBlock, WireGenerateImageExecutionPlan, WireGenerateImageRequest,
    WireImageGenerationToolResult, WireImageOperationPhase, WireModelRoutingApprovalPhase,
    WireModelRoutingApprovalRequest, WireScopedModelOverride, WireSessionHistory,
    WireSessionMessage, WireSessionModelRoutingStatus, WireSwitchTurnControlResult,
    WireSwitchTurnIntent, WireSwitchTurnPhase,
};
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::{
    ApprovalId, AssistantImageId, AssistantImageRef, BlobId, BlobRef, BlockAssistantMessage,
    GenerateImageExecutionPlan, GenerateImageRequest, ImageContinuityTokenSupport,
    ImageFormatPreference, ImageGenerationBackendKind, ImageGenerationIntent,
    ImageGenerationTargetCapabilities, ImageGenerationTargetPreference, ImageGenerationToolResult,
    ImageOperationApprovalReason, ImageOperationDenialReason, ImageOperationId,
    ImageOperationPhase, ImageOperationTerminalClass, ImageQualityPreference, ImageSizePreference,
    MediaType, Message, ModelRoutingApprovalPhase, ModelRoutingApprovalRequest,
    ModelRoutingApprovalTerminalClass, PromptSource, PromptText, ProviderId, ProviderImageMetadata,
    ProviderTextDisposition, RevisedPromptDisposition, ScopedModelOverride, ScopedModelOverrideId,
    ScopedModelOverrideKind, ScopedModelOverrideSummary, SessionHistoryPage, StopReason,
    SwitchTurnApprovalReason, SwitchTurnControlResult, SwitchTurnDenialReason, SwitchTurnDuration,
    SwitchTurnIntent, SwitchTurnOrigin, SwitchTurnPhase, SwitchTurnPolicyReason,
    SwitchTurnReasonText, SwitchTurnReasonTextDisposition, SwitchTurnRequestId,
    SwitchTurnTerminalClass, TextArtifactRef, ToolCallId, TopologyEpoch,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

fn uuid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn roundtrip<T>(value: T)
where
    T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let encoded = serde_json::to_string(&value).unwrap();
    let decoded: T = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, value);
}

#[test]
fn assistant_image_block_projects_to_wire_image_not_unknown() {
    let block = meerkat_core::AssistantBlock::Image {
        image_id: AssistantImageId::new(uuid(1)),
        blob_ref: BlobRef {
            blob_id: BlobId::new("generated-1"),
            media_type: "image/png".into(),
        },
        media_type: MediaType::new("image/png"),
        width: 1024,
        height: 1024,
        revised_prompt: RevisedPromptDisposition::NotRequested,
        meta: ProviderImageMetadata::NotEmitted,
    };

    let wire = WireAssistantBlock::from(block);
    assert!(matches!(wire, WireAssistantBlock::Image { .. }));

    let encoded = serde_json::to_string(&wire).unwrap();
    assert!(encoded.contains("\"block_type\":\"image\""));
    let decoded: WireAssistantBlock = serde_json::from_str(&encoded).unwrap();
    assert!(matches!(decoded, WireAssistantBlock::Image { .. }));
}

#[test]
fn assistant_image_block_remains_typed_in_wire_history() -> Result<(), String> {
    let page = SessionHistoryPage {
        session_id: meerkat_core::SessionId::new(),
        offset: 0,
        limit: Some(50),
        message_count: 1,
        has_more: false,
        messages: vec![Message::BlockAssistant(BlockAssistantMessage::new(
            vec![meerkat_core::AssistantBlock::Image {
                image_id: AssistantImageId::new(uuid(2)),
                blob_ref: BlobRef {
                    blob_id: BlobId::new("generated-history-image"),
                    media_type: "image/png".into(),
                },
                media_type: MediaType::new("image/png"),
                width: 1024,
                height: 1024,
                revised_prompt: RevisedPromptDisposition::NotRequested,
                meta: ProviderImageMetadata::NotEmitted,
            }],
            StopReason::EndTurn,
        ))],
    };

    let wire = WireSessionHistory::from(page);
    let Some(message) = wire.messages.first() else {
        return Err("expected block assistant history message, got none".to_string());
    };
    let WireSessionMessage::BlockAssistant { blocks, .. } = message else {
        return Err(format!(
            "expected block assistant history message, got {message:?}"
        ));
    };
    let Some(block) = blocks.first() else {
        return Err("expected assistant image block, got none".to_string());
    };
    let WireAssistantBlock::Image {
        blob_ref,
        media_type,
        width,
        height,
        ..
    } = block
    else {
        return Err(format!("expected assistant image block, got {block:?}"));
    };
    assert_eq!(blob_ref.blob_id.as_str(), "generated-history-image");
    assert_eq!(media_type.as_str(), "image/png");
    assert_eq!((*width, *height), (1024, 1024));
    Ok(())
}

#[test]
fn image_generation_wire_aliases_roundtrip() {
    let request: WireGenerateImageRequest = GenerateImageRequest::new(
        ImageGenerationIntent::Generate {
            prompt: PromptText::new("draw a schema boundary").unwrap(),
            prompt_source: PromptSource::ModelDistilled {
                tool_call_id: ToolCallId::new("tc_image"),
            },
            reference_images: Vec::new(),
        },
        ImageGenerationTargetPreference::ProviderDefault {
            provider: ProviderId::new("openai"),
        },
        ImageSizePreference::Square1024,
        ImageQualityPreference::Low,
        ImageFormatPreference::Png,
        NonZeroU32::new(1).unwrap(),
    )
    .unwrap();
    let encoded = serde_json::to_string(&request).unwrap();
    let decoded: WireGenerateImageRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, request);

    let plan: WireGenerateImageExecutionPlan = GenerateImageExecutionPlan {
        provider: ProviderId::new("provider-a"),
        backend: ImageGenerationBackendKind::HostedTool,
        max_count: NonZeroU32::new(4).unwrap(),
        capabilities: ImageGenerationTargetCapabilities {
            hosted_image_generation_tool: true,
            native_image_output: false,
            custom_tools: true,
            image_search_grounding: false,
            image_continuity_tokens: ImageContinuityTokenSupport::SameProviderOnly,
        },
        requires_scoped_override: false,
        provider_plan: serde_json::json!({"tool_name": "image_generation"}),
    };
    let encoded = serde_json::to_string(&plan).unwrap();
    let decoded: WireGenerateImageExecutionPlan = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, plan);

    let edits_plan: WireGenerateImageExecutionPlan = GenerateImageExecutionPlan {
        provider: ProviderId::new("provider-a"),
        backend: ImageGenerationBackendKind::ProviderApi,
        max_count: NonZeroU32::new(1).unwrap(),
        capabilities: ImageGenerationTargetCapabilities {
            hosted_image_generation_tool: false,
            native_image_output: false,
            custom_tools: false,
            image_search_grounding: false,
            image_continuity_tokens: ImageContinuityTokenSupport::Unsupported,
        },
        requires_scoped_override: false,
        provider_plan: serde_json::json!({"endpoint": "edits"}),
    };
    roundtrip(edits_plan);

    let gemini_plan: WireGenerateImageExecutionPlan = GenerateImageExecutionPlan {
        provider: ProviderId::new("provider-b"),
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
            "projection_snapshot_id": meerkat_core::ProjectionSnapshotId::new(uuid(2))
        }),
    };
    let encoded = serde_json::to_string(&gemini_plan).unwrap();
    let decoded: WireGenerateImageExecutionPlan = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, gemini_plan);

    let result: WireImageGenerationToolResult = ImageGenerationToolResult {
        operation_id: ImageOperationId::new(uuid(3)),
        terminal: ImageOperationTerminalClass::Generated,
        images: vec![AssistantImageRef {
            image_id: AssistantImageId::new(uuid(4)),
            blob_ref: BlobRef {
                blob_id: BlobId::new("generated-4"),
                media_type: "image/png".into(),
            },
            media_type: MediaType::new("image/png"),
            width: 1024,
            height: 1024,
        }],
        provider_text: ProviderTextDisposition::Captured {
            text_artifact_ref: TextArtifactRef::new("caption-1"),
        },
        revised_prompt: RevisedPromptDisposition::Unchanged,
        native_metadata: ProviderImageMetadata::NotEmitted,
        warnings: Vec::new(),
    };
    let encoded = serde_json::to_string(&result).unwrap();
    let decoded: WireImageGenerationToolResult = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, result);
}

#[test]
fn model_routing_wire_aliases_roundtrip() {
    let switch_intent: WireSwitchTurnIntent = SwitchTurnIntent {
        target_model: ModelId::new("gemini-2.5-flash"),
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    };
    roundtrip(switch_intent);

    let applied: WireSwitchTurnControlResult = SwitchTurnControlResult::Applied {
        request_id: SwitchTurnRequestId::new(uuid(10)),
        target_model: ModelId::new("gpt-5.5"),
        duration: SwitchTurnDuration::Finite {
            duration: meerkat_core::FiniteScopedTurnDuration::OneTurn,
        },
    };
    roundtrip(applied);
    let denied: WireSwitchTurnControlResult = SwitchTurnControlResult::Denied {
        request_id: SwitchTurnRequestId::new(uuid(11)),
        reason: SwitchTurnDenialReason::UnsupportedModel,
    };
    roundtrip(denied);

    let approval: WireModelRoutingApprovalRequest = ModelRoutingApprovalRequest::SwitchTurn {
        request_id: SwitchTurnRequestId::new(uuid(5)),
        reason: meerkat_core::SwitchTurnApprovalReason::CrossProvider,
    };
    let encoded = serde_json::to_string(&approval).unwrap();
    let decoded: WireModelRoutingApprovalRequest = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, approval);

    let phase: WireSwitchTurnPhase = SwitchTurnPhase::PendingForBoundary;
    let encoded = serde_json::to_string(&phase).unwrap();
    let decoded: WireSwitchTurnPhase = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, phase);
    let terminal_phase: WireSwitchTurnPhase = SwitchTurnPhase::Terminal {
        terminal: SwitchTurnTerminalClass::Denied {
            reason: SwitchTurnDenialReason::ApprovalRequiredButUnavailable,
        },
    };
    roundtrip(terminal_phase);

    let approval_phase: WireModelRoutingApprovalPhase = ModelRoutingApprovalPhase::Terminal {
        terminal: ModelRoutingApprovalTerminalClass::DeniedByUser,
    };
    roundtrip(approval_phase);

    let image_phase: WireImageOperationPhase = ImageOperationPhase::Terminal {
        terminal: ImageOperationTerminalClass::Denied {
            reason: ImageOperationDenialReason::DeniedDuringApproval {
                approvable: ImageOperationApprovalReason::CrossProvider,
            },
        },
    };
    roundtrip(image_phase);

    let override_state: WireScopedModelOverride = ScopedModelOverride {
        id: ScopedModelOverrideId::new(uuid(6)),
        kind: ScopedModelOverrideKind::ImageOperation {
            operation_id: ImageOperationId::new(uuid(7)),
        },
        previous_effective_model: ModelId::new("previous"),
        baseline_model_snapshot: ModelId::new("baseline"),
        target_model: ModelId::new("target"),
        topology_epoch: TopologyEpoch(9),
    };
    let encoded = serde_json::to_string(&override_state).unwrap();
    let decoded: WireScopedModelOverride = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, override_state);

    let status: WireSessionModelRoutingStatus = WireSessionModelRoutingStatus::new(
        ModelId::new("baseline"),
        None,
        Some(ScopedModelOverrideSummary {
            id: ScopedModelOverrideId::new(uuid(8)),
            kind: ScopedModelOverrideKind::ImageOperation {
                operation_id: ImageOperationId::new(uuid(9)),
            },
            target_model: ModelId::new("operation"),
            topology_epoch: TopologyEpoch(10),
        }),
        None,
    );
    let encoded = serde_json::to_string(&status).unwrap();
    let decoded: WireSessionModelRoutingStatus = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, status);
}

#[test]
fn wire_declared_variant_families_roundtrip() {
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
        let phase: WireImageOperationPhase = phase;
        roundtrip(phase);
    }
    let phase: WireImageOperationPhase = ImageOperationPhase::AwaitingApproval {
        approval_id: ApprovalId::new(),
    };
    roundtrip(phase);
    let phase: WireImageOperationPhase = ImageOperationPhase::Terminal {
        terminal: ImageOperationTerminalClass::Generated,
    };
    roundtrip(phase);

    for phase in [
        SwitchTurnPhase::Requested,
        SwitchTurnPhase::Validating,
        SwitchTurnPhase::PendingForBoundary,
        SwitchTurnPhase::ApplyingFiniteOverride,
        SwitchTurnPhase::ApplyingPersistentReconfigure,
        SwitchTurnPhase::ActiveFiniteOverride,
        SwitchTurnPhase::RestoringFiniteOverride,
    ] {
        let phase: WireSwitchTurnPhase = phase;
        roundtrip(phase);
    }
    let phase: WireSwitchTurnPhase = SwitchTurnPhase::AwaitingApproval {
        approval_id: ApprovalId::new(),
    };
    roundtrip(phase);
    let phase: WireSwitchTurnPhase = SwitchTurnPhase::Terminal {
        terminal: SwitchTurnTerminalClass::PersistentReconfigureApplied,
    };
    roundtrip(phase);

    for terminal in [
        SwitchTurnTerminalClass::ConsumedAndRestored,
        SwitchTurnTerminalClass::InterruptedAndRestored,
        SwitchTurnTerminalClass::PersistentReconfigureApplied,
        SwitchTurnTerminalClass::PersistentReconfigureFailed,
    ] {
        roundtrip(terminal);
    }
    roundtrip(SwitchTurnTerminalClass::Denied {
        reason: SwitchTurnDenialReason::ProjectionUnsupported,
    });
    roundtrip(SwitchTurnTerminalClass::RestoreFailed {
        trigger: meerkat_core::SwitchTurnRestoreTrigger::Interrupted,
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
        approvable: SwitchTurnApprovalReason::SafetyHold,
    });

    for terminal in [
        ModelRoutingApprovalTerminalClass::Approved,
        ModelRoutingApprovalTerminalClass::DeniedByUser,
        ModelRoutingApprovalTerminalClass::Expired,
        ModelRoutingApprovalTerminalClass::Interrupted,
        ModelRoutingApprovalTerminalClass::SessionArchived,
        ModelRoutingApprovalTerminalClass::SurfaceDetached,
    ] {
        let phase: WireModelRoutingApprovalPhase = ModelRoutingApprovalPhase::Terminal { terminal };
        roundtrip(phase);
    }
    roundtrip::<WireModelRoutingApprovalPhase>(ModelRoutingApprovalPhase::Pending);
    roundtrip::<WireModelRoutingApprovalPhase>(ModelRoutingApprovalPhase::PresentedToUser);

    for reason in [
        SwitchTurnApprovalReason::CrossProvider,
        SwitchTurnApprovalReason::CostExceedsThreshold,
        SwitchTurnApprovalReason::SafetyHold,
        SwitchTurnApprovalReason::UntilChangedFromModelOrigin,
        SwitchTurnApprovalReason::RealtimeDetachRequired,
    ] {
        let request: WireModelRoutingApprovalRequest = ModelRoutingApprovalRequest::SwitchTurn {
            request_id: SwitchTurnRequestId::new(uuid(20)),
            reason,
        };
        roundtrip(request);
    }
    for reason in [
        ImageOperationApprovalReason::CrossProvider,
        ImageOperationApprovalReason::CostExceedsThreshold,
        ImageOperationApprovalReason::SafetyHold,
        ImageOperationApprovalReason::RealtimeDetachRequired,
    ] {
        let request: WireModelRoutingApprovalRequest =
            ModelRoutingApprovalRequest::ImageOperation {
                operation_id: ImageOperationId::new(uuid(21)),
                reason,
            };
        roundtrip(request);
    }

    for policy in [
        SwitchTurnPolicyReason::BudgetDowngrade,
        SwitchTurnPolicyReason::SafetyHandoff,
        SwitchTurnPolicyReason::ReleaseTemporaryHold,
    ] {
        let intent: WireSwitchTurnIntent = SwitchTurnIntent {
            target_model: ModelId::new("policy-target"),
            duration: SwitchTurnDuration::UntilChanged,
            origin: SwitchTurnOrigin::SystemPolicy { reason: policy },
        };
        roundtrip(intent);
    }
    let intent: WireSwitchTurnIntent = SwitchTurnIntent {
        target_model: ModelId::new("user-target"),
        duration: SwitchTurnDuration::Finite {
            duration: meerkat_core::FiniteScopedTurnDuration::Turns {
                turns: NonZeroU32::new(2).unwrap(),
            },
        },
        origin: SwitchTurnOrigin::User {
            reason: SwitchTurnReasonTextDisposition::Provided {
                reason: SwitchTurnReasonText::new("try this briefly").unwrap(),
            },
        },
    };
    roundtrip(intent);
}

#[cfg(feature = "schema")]
#[test]
fn schema_entrypoint_contains_image_generation_contracts() {
    let schema = serde_json::to_value(schemars::schema_for!(WireGenerateImageRequest)).unwrap();
    assert!(schema.to_string().contains("GenerateImageRequest"));

    let schema = serde_json::to_value(schemars::schema_for!(WireAssistantBlock)).unwrap();
    assert!(schema.to_string().contains("image_id"));
}
