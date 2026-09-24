//! Permanent model-switch request builtin.
//!
//! # What this tool does, precisely
//!
//! It stages a request. That is all it does.
//!
//! The model names a target; the tool validates that target against the set of
//! models this session can actually reach, writes the intent into a run-local
//! slot, and returns. It performs no routing, touches no provider, holds no
//! credential, and calls into no runtime. Every provider call in the run that
//! invoked it — including the one that reads this tool's own result — is served
//! by the model that was already active.
//!
//! # Why it cannot do more than that
//!
//! The tool executes inside a live turn, on the session actor, while that turn
//! holds the session. Switching identity from here would mean either mutating
//! the session out from under an in-flight turn, or calling back into the
//! runtime that is currently waiting on this very tool. The first corrupts the
//! transcript's provenance; the second deadlocks. The request therefore has to
//! outlive the run that made it, which is exactly what the durable handoff log
//! and the pre-dequeue realization seam exist for.
//!
//! # What the model is told
//!
//! The result says the switch is *staged*, and says plainly that it takes
//! effect on the next turn only if this run completes. Overstating it would be
//! a lie in the failure case: a run that errors, is interrupted, or is denied
//! by a hook commits nothing, and the next turn continues on the current model.
//!
//! # Input surface
//!
//! One field: `target_model`, narrowed by an enum to the available set.
//! Provider, provider parameters, credentials, accounts, and realtime policy
//! are deliberately absent. They are resolved fresh at realization from the
//! session's own identity, so the model cannot use this tool to reach a
//! provider or credential it was not already entitled to.

use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};
use async_trait::async_trait;
use meerkat_core::ToolMutationClass;
use meerkat_core::image_generation::SwitchTurnRequestId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_handoff_staging::{
    ModelRoutingHandoffStageError, ModelRoutingHandoffStageOutcome, ModelRoutingHandoffStagingSlot,
};
use meerkat_core::types::{ToolDef, ToolProvenance, ToolSourceKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

pub const BRAIN_SWAP_TOOL_NAME: &str = "brain_swap";

const BRAIN_SWAP_TOOL_DOCUMENTATION: &str = r#"Request that this session switch to a different model for subsequent turns.

The switch does NOT take effect during the current turn or anywhere else in this run: every remaining model call in this run, including the one that reads this result, uses the current model. The request is staged now and applied before the next input is processed, and only if this run finishes cleanly. A run that fails, is interrupted, or is denied changes nothing.

Choose only the target model. Provider routing, provider parameters, account selection, and credentials are resolved behind the runtime and cannot be supplied here.

Request shape:
{"target_model":"gpt-5.5"}"#;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct BrainSwapToolArgs {
    /// Model to use for subsequent turns.
    target_model: String,
}

/// What the model is told after staging.
///
/// `already_staged` is reported distinctly from `staged` rather than being
/// flattened, so a model that restates its choice can see that it did not
/// create a second request.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum BrainSwapToolOutcome {
    Staged {
        target_model: String,
        request_id: SwitchTurnRequestId,
        effective: &'static str,
    },
    AlreadyStaged {
        target_model: String,
        request_id: SwitchTurnRequestId,
        effective: &'static str,
    },
}

/// The one sentence describing when a staged request becomes real.
const BRAIN_SWAP_EFFECTIVE_DESCRIPTION: &str =
    "applied before the next input is processed, only if this run completes successfully";

/// Staging-only permanent model-switch builtin.
pub struct BrainSwapTool {
    staging: Arc<ModelRoutingHandoffStagingSlot>,
    models: BTreeSet<String>,
}

impl BrainSwapTool {
    /// Build the tool over the exact set of models this session can reach.
    ///
    /// Duplicates in `models` collapse: availability is resolved from several
    /// provider routes that can legitimately land on the same model id, and the
    /// question this tool answers is "which distinct models can I ask for",
    /// not "how many routes exist".
    pub fn new(
        staging: Arc<ModelRoutingHandoffStagingSlot>,
        models: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            staging,
            models: models.into_iter().collect(),
        }
    }

    /// How many DISTINCT models this session could switch between.
    ///
    /// The registration gate reads this. One model is not a choice, so the
    /// tool is not offered at all in that case rather than being offered and
    /// always refusing.
    #[must_use]
    pub fn available_model_count(&self) -> usize {
        self.models.len()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for BrainSwapTool {
    fn name(&self) -> &'static str {
        BRAIN_SWAP_TOOL_NAME
    }

    fn def(&self) -> ToolDef {
        let mut input_schema = crate::schema::schema_for::<BrainSwapToolArgs>();
        input_schema["properties"]["target_model"]["enum"] = Value::Array(
            self.models
                .iter()
                .map(|model| Value::String(model.clone()))
                .collect(),
        );
        ToolDef {
            name: self.name().into(),
            description: BRAIN_SWAP_TOOL_DOCUMENTATION.to_string(),
            input_schema,
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    /// Mutating, despite touching nothing outside process memory when it runs.
    ///
    /// The classification describes what the call ultimately causes, not what
    /// its own body does: a staged request that survives to a clean run
    /// boundary durably changes which model answers afterwards. Declaring this
    /// `ReadOnly` would let a read-only launch hand the model a lever over its
    /// own successor, which is precisely what a read-only claim promises it
    /// cannot do.
    fn mutation_class(&self) -> ToolMutationClass {
        ToolMutationClass::Mutating
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        let args: BrainSwapToolArgs = serde_json::from_value(args)
            .map_err(|error| BuiltinToolError::invalid_args(error.to_string()))?;
        if !self.models.contains(&args.target_model) {
            return Err(BuiltinToolError::invalid_args(format!(
                "model '{}' is not available to this session",
                args.target_model
            )));
        }
        let outcome = self
            .staging
            .stage(
                SwitchTurnRequestId::new(uuid::Uuid::new_v4()),
                ModelId::new(args.target_model.clone()),
            )
            .map_err(|error| match error {
                ModelRoutingHandoffStageError::ConflictingTarget { .. } => {
                    BuiltinToolError::invalid_args(error.to_string())
                }
                ModelRoutingHandoffStageError::SlotUnusable => {
                    BuiltinToolError::execution_failed(error.to_string())
                }
            })?;
        let outcome = match outcome {
            ModelRoutingHandoffStageOutcome::Staged { request_id } => {
                BrainSwapToolOutcome::Staged {
                    target_model: args.target_model,
                    request_id,
                    effective: BRAIN_SWAP_EFFECTIVE_DESCRIPTION,
                }
            }
            ModelRoutingHandoffStageOutcome::AlreadyStaged { request_id } => {
                BrainSwapToolOutcome::AlreadyStaged {
                    target_model: args.target_model,
                    request_id,
                    effective: BRAIN_SWAP_EFFECTIVE_DESCRIPTION,
                }
            }
        };
        serde_json::to_value(outcome)
            .map(ToolOutput::Json)
            .map_err(|error| BuiltinToolError::execution_failed(error.to_string()))
    }
}
