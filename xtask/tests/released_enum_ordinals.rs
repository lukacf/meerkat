//! Declaration-order discriminants from v0.8.36
//! (09c1174e3b13df8ad71b3cd07e533c71de5f8b90). Add variants after the released
//! prefix; moving a variant changes its implicit Rust discriminant even when
//! its tagged JSON representation is unchanged.

use anyhow::{Context, Result};

fn assert_released_ordinals(path: &str, name: &str, released: &[&str]) -> Result<()> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(path);
    let source = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read enum source {}", path.display()))?;
    let file = syn::parse_file(&source).context("invalid Rust source")?;
    let item = file
        .items
        .into_iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == name => Some(item),
            _ => None,
        })
        .with_context(|| format!("public enum {name} is missing"))?;
    assert!(
        item.variants.len() >= released.len(),
        "{name} lost released variants"
    );
    for (ordinal, expected) in released.iter().enumerate() {
        let variant = &item.variants[ordinal];
        assert_eq!(
            variant.ident.to_string(),
            *expected,
            "{name} changed the v0.8.36 variant at ordinal {ordinal}"
        );
        assert!(
            variant.discriminant.is_none(),
            "{name}::{expected} no longer uses its released implicit discriminant"
        );
        assert!(
            variant
                .attrs
                .iter()
                .all(|attr| !attr.path().is_ident("cfg") && !attr.path().is_ident("cfg_attr")),
            "{name}::{expected} must keep its ordinal across feature sets"
        );
    }
    Ok(())
}

#[test]
fn agent_event_preserves_v0_8_36_ordinals() -> Result<()> {
    assert_released_ordinals(
        "meerkat-core/src/event.rs",
        "AgentEvent",
        &[
            "RunStarted",
            "RunCompleted",
            "ExtractionSucceeded",
            "ExtractionFailed",
            "RunFailed",
            "HookStarted",
            "HookCompleted",
            "HookFailed",
            "HookDenied",
            "TurnStarted",
            "ReasoningDelta",
            "ReasoningComplete",
            "TextDelta",
            "TextComplete",
            "ServerToolContent",
            "AssistantImageAppended",
            "ToolCallRequested",
            "ToolResultReceived",
            "TurnCompleted",
            "ToolExecutionStarted",
            "ToolExecutionCompleted",
            "ToolExecutionTimedOut",
            "CompactionStarted",
            "CompactionCompleted",
            "CompactionFailed",
            "BudgetWarning",
            "Retrying",
            "SkillsResolved",
            "SkillResolutionFailed",
            "InteractionComplete",
            "InteractionCallbackPending",
            "InteractionFailed",
            "StreamTruncated",
            "ToolConfigChanged",
            "BackgroundJobCompleted",
            "TranscriptRewriteCommitted",
            "TranscriptRewriteAuditReceiptCommitted",
            "ProviderCacheBreakpointsDiscarded",
            "PeerContentIngested",
            "TurnUsageAccountingUnmeasured",
            "TurnUsageAccountingIdentityDisputed",
        ],
    )
}

#[test]
fn agent_error_reason_preserves_v0_8_36_ordinals() -> Result<()> {
    assert_released_ordinals(
        "meerkat-core/src/event.rs",
        "AgentErrorReason",
        &[
            "LlmRateLimited",
            "LlmContextExceeded",
            "LlmAuthError",
            "LlmInvalidModel",
            "LlmProviderError",
            "LlmNetworkTimeout",
            "LlmCallTimeout",
            "HookDenied",
            "HookTimeout",
            "HookExecutionFailed",
            "HookConfigInvalid",
            "StructuredOutputValidationFailed",
            "InvalidOutputSchema",
            "AuthReauthRequired",
            "CallbackPending",
            "TurnTerminalCause",
        ],
    )
}

#[test]
fn diagnostic_code_preserves_v0_8_36_ordinals() -> Result<()> {
    assert_released_ordinals(
        "meerkat-mob/src/validate.rs",
        "DiagnosticCode",
        &[
            "MissingSkillRef",
            "MissingOrchestratorProfile",
            "InvalidProfileName",
            "InvalidWiringProfile",
            "EmptyProfiles",
            "MissingExternalBackendConfig",
            "InvalidExternalBackendConfig",
            "FlowCycleDetected",
            "FlowUnknownStep",
            "FlowUnknownRole",
            "FlowDepthExceeded",
            "TopologyUnknownRole",
            "QuorumInvalid",
            "BranchGroupEmpty",
            "BranchStepMissingCondition",
            "BranchStepConflictingDeps",
            "BranchJoinWithoutBranch",
            "ReservedSystemIdentifier",
            "InvalidInlinePeerNotificationThreshold",
            "UnknownModel",
            "InvalidCustomModel",
            "InvalidImageGenerationProvider",
            "JsonOutputWithoutSchema",
            "UnknownProfileKey",
        ],
    )
}
