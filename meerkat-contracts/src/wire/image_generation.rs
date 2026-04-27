//! Wire-facing image-generation and scoped model-routing contracts.
//!
//! These are type aliases over the core domain contracts for the Phase 0
//! representation boundary. Runtime semantics are still owned by
//! `MeerkatMachine`; this module makes the contracts visible to surfaces and
//! schema emission without duplicating semantic fields.

pub type WireAssistantImageRef = meerkat_core::AssistantImageRef;
pub type WireGenerateImageRequest = meerkat_core::GenerateImageRequest;
pub type WireGenerateImageExecutionPlan = meerkat_core::GenerateImageExecutionPlan;
pub type WireImageGenerationToolResult = meerkat_core::ImageGenerationToolResult;
pub type WireImageOperationPhase = meerkat_core::ImageOperationPhase;
pub type WireSwitchTurnIntent = meerkat_core::SwitchTurnIntent;
pub type WireSwitchTurnControlResult = meerkat_core::SwitchTurnControlResult;
pub type WireSwitchTurnPhase = meerkat_core::SwitchTurnPhase;
pub type WireModelRoutingApprovalPhase = meerkat_core::ModelRoutingApprovalPhase;
pub type WireModelRoutingApprovalRequest = meerkat_core::ModelRoutingApprovalRequest;
pub type WireScopedModelOverride = meerkat_core::ScopedModelOverride;
pub type WireSessionModelRoutingStatus = meerkat_core::SessionModelRoutingStatus;
