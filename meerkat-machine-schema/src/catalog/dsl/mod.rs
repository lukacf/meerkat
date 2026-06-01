//! DSL-generated machine schemas.
//!
//! These modules contain `machine!` invocations that generate the same
//! `MachineSchema` values as the hand-written catalog entries. They use
//! `rust: "self"` so the generated `schema()` function references
//! `crate::MachineSchema` instead of `meerkat_machine_schema::MachineSchema`.
#![allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::cmp_owned,
    clippy::assign_op_pattern
)]

/// Extension trait providing `.get()` on Option to support the `option_value`
/// schema pattern (`Expr::MapGet { map: Field(...), key: String("value") }`).
/// In the runtime dispatch code, `.get("value")` extracts the inner value.
/// This is only used by the generated dispatch code in this crate (which is
/// dead code — only the `schema()` function is called).
pub trait OptionValueExt<T: Clone> {
    fn get(&self, _key: &str) -> T;
}
impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
    fn get(&self, _key: &str) -> T {
        self.clone().unwrap_or_default()
    }
}

impl<T: Clone + Default> OptionValueExt<T> for Option<&T> {
    fn get(&self, _key: &str) -> T {
        self.cloned().unwrap_or_default()
    }
}

pub mod approval_lifecycle;
pub mod auth_machine;
pub mod meerkat_machine;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;
pub mod session_document;
pub mod session_persistence_version_authority;
pub mod session_turn_admission;
pub mod work_attention_lifecycle;
pub mod workgraph_lifecycle;

use crate::identity::InputVariantId;
use crate::{
    MachineSchema, NamedTypeBinding, RustBinding, TypePathEnumStructuralVariant,
    TypePathStructField,
};

pub struct MachineSchemaMetadata {
    pub named_types: Vec<NamedTypeBinding>,
    pub runtime_internal_inputs: Vec<InputVariantId>,
    pub ci_step_limit: Option<u32>,
}

impl MachineSchemaMetadata {
    pub fn attach_to(self, mut schema: MachineSchema) -> MachineSchema {
        schema.named_types = self.named_types;
        schema.runtime_internal_inputs = self.runtime_internal_inputs;
        schema.ci_step_limit = self.ci_step_limit;
        schema
    }

    pub fn with_ci_step_limit(mut self, ci_step_limit: u32) -> Self {
        self.ci_step_limit = Some(ci_step_limit);
        self
    }
}

pub const AUTH_MACHINE_PRODUCTION_RUST_CRATE: &str = "meerkat-runtime";
pub const AUTH_MACHINE_PRODUCTION_RUST_MODULE: &str = "auth_machine::dsl";
pub const APPROVAL_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-core";
pub const APPROVAL_LIFECYCLE_PRODUCTION_RUST_MODULE: &str = "generated::approval_lifecycle";
pub const MEERKAT_MACHINE_PRODUCTION_RUST_CRATE: &str = "meerkat-runtime";
pub const MEERKAT_MACHINE_PRODUCTION_RUST_MODULE: &str = "meerkat_machine::dsl";
pub const MOB_MACHINE_PRODUCTION_RUST_CRATE: &str = "meerkat-mob";
pub const MOB_MACHINE_PRODUCTION_RUST_MODULE: &str = "machines::mob_machine";
pub const SCHEDULE_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-schedule";
pub const SCHEDULE_LIFECYCLE_PRODUCTION_RUST_MODULE: &str = "machines::schedule_lifecycle";
pub const OCCURRENCE_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-schedule";
pub const OCCURRENCE_LIFECYCLE_PRODUCTION_RUST_MODULE: &str = "machines::occurrence_lifecycle";
pub const SESSION_DOCUMENT_PRODUCTION_RUST_CRATE: &str = "meerkat-core";
pub const SESSION_DOCUMENT_PRODUCTION_RUST_MODULE: &str = "generated::session_document";
pub const SESSION_TURN_ADMISSION_PRODUCTION_RUST_CRATE: &str = "meerkat-session";
pub const SESSION_TURN_ADMISSION_PRODUCTION_RUST_MODULE: &str = "generated::session_turn_admission";
pub const SESSION_PERSISTENCE_VERSION_AUTHORITY_PRODUCTION_RUST_CRATE: &str = "meerkat-core";
pub const SESSION_PERSISTENCE_VERSION_AUTHORITY_PRODUCTION_RUST_MODULE: &str =
    "generated::session_persistence_version_authority";
pub const WORKGRAPH_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-workgraph";
pub const WORKGRAPH_LIFECYCLE_PRODUCTION_RUST_MODULE: &str = "machines::workgraph_lifecycle";
pub const WORK_ATTENTION_LIFECYCLE_PRODUCTION_RUST_CRATE: &str = "meerkat-workgraph";
pub const WORK_ATTENTION_LIFECYCLE_PRODUCTION_RUST_MODULE: &str =
    "machines::work_attention_lifecycle";

fn with_production_rust_binding(
    mut schema: MachineSchema,
    crate_name: &str,
    module: &str,
) -> MachineSchema {
    schema.rust = RustBinding {
        crate_name: crate_name.to_owned(),
        module: module.to_owned(),
    };
    schema
}

trait RuntimeInternalInputVariant: Copy {
    fn input_variant_id(self) -> InputVariantId;
}

macro_rules! runtime_internal_inputs {
    (
        $type_name:ident,
        $const_name:ident,
        $variant_module:ident::$variant_type:ident,
        [$($variant:ident),+ $(,)?]
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum $type_name {
            $($variant),+
        }

        const $const_name: &[$type_name] = &[
            $($type_name::$variant),+
        ];

        impl $type_name {
            fn input_variant(self) -> $variant_module::$variant_type {
                match self {
                    $(Self::$variant => $variant_module::$variant_type::$variant,)+
                }
            }
        }

        impl RuntimeInternalInputVariant for $type_name {
            fn input_variant_id(self) -> InputVariantId {
                InputVariantId::from_trusted_catalog_literal(match self {
                    $(Self::$variant => stringify!($variant),)+
                })
            }
        }
    };
}

fn input_variant_ids<T: RuntimeInternalInputVariant>(
    variants: &'static [T],
) -> Vec<InputVariantId> {
    variants
        .iter()
        .copied()
        .map(RuntimeInternalInputVariant::input_variant_id)
        .collect()
}

fn machine_schema_metadata(
    named_types: Vec<NamedTypeBinding>,
    runtime_internal_inputs: Vec<InputVariantId>,
) -> MachineSchemaMetadata {
    MachineSchemaMetadata {
        named_types,
        runtime_internal_inputs,
        ci_step_limit: None,
    }
}

pub fn dsl_auth_machine() -> MachineSchema {
    auth_machine_schema_metadata().attach_to(auth_machine::AuthMachineState::schema())
}

pub fn dsl_auth_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_auth_machine(),
        AUTH_MACHINE_PRODUCTION_RUST_CRATE,
        AUTH_MACHINE_PRODUCTION_RUST_MODULE,
    )
}

pub fn dsl_approval_lifecycle_machine() -> MachineSchema {
    approval_lifecycle_schema_metadata()
        .attach_to(approval_lifecycle::ApprovalLifecycleMachineState::schema())
}

pub fn dsl_approval_lifecycle_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_approval_lifecycle_machine(),
        APPROVAL_LIFECYCLE_PRODUCTION_RUST_CRATE,
        APPROVAL_LIFECYCLE_PRODUCTION_RUST_MODULE,
    )
}

pub fn approval_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string_enum(
                "ApprovalLifecycleStatus",
                &["Pending", "Approved", "Denied", "Expired", "Cancelled"],
            ),
            NamedTypeBinding::string_enum("ApprovalLifecycleDecision", &["Approve", "Deny"]),
            NamedTypeBinding::string_enum(
                "ApprovalLifecycleRejectionReason",
                &[
                    "NotFound",
                    "AlreadyExists",
                    "AlreadyDecided",
                    "Expired",
                    "InvalidDecision",
                    "EmptyAllowedDecisions",
                    "InvalidRestoredRecord",
                ],
            ),
        ],
        Vec::new(),
    )
}

pub fn auth_machine_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string_enum(
                "AuthLifecyclePhase",
                &[
                    "Valid",
                    "Expiring",
                    "Expired",
                    "Refreshing",
                    "ReauthRequired",
                    "Released",
                ],
            ),
            NamedTypeBinding::string_enum(
                "CredentialUseIntent",
                &["UseCredential", "HoldAuthority", "BeginRefresh"],
            ),
            NamedTypeBinding::string_enum(
                "CredentialUseDisposition",
                &[
                    "Authorized",
                    "RefreshRequired",
                    "ReauthRequired",
                    "LeaseAbsent",
                    "AlreadyRefreshing",
                ],
            ),
        ],
        vec![
            InputVariantId::from_trusted_catalog_literal("RestoreAuthoritySnapshot"),
            InputVariantId::from_trusted_catalog_literal("RestoreCredentialLifecycleSnapshot"),
            InputVariantId::from_trusted_catalog_literal("RestoreOAuthBrowserFlow"),
            InputVariantId::from_trusted_catalog_literal("RestoreOAuthDeviceFlow"),
            InputVariantId::from_trusted_catalog_literal("RestoreOAuthDevicePoll"),
            InputVariantId::from_trusted_catalog_literal("ResolveCredentialUseAdmission"),
        ],
    )
}

pub fn dsl_meerkat_machine() -> MachineSchema {
    meerkat_machine_schema_metadata().attach_to(meerkat_machine::MeerkatMachineState::schema())
}

/// Canonical SessionDocumentMachine — owns per-session session-document
/// lifecycle truth (currently the first-turn region) in its own `Map` state.
pub fn dsl_session_document_machine() -> MachineSchema {
    session_document_schema_metadata()
        .attach_to(session_document::SessionDocumentMachineState::schema())
}

pub fn dsl_session_document_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_session_document_machine(),
        SESSION_DOCUMENT_PRODUCTION_RUST_CRATE,
        SESSION_DOCUMENT_PRODUCTION_RUST_MODULE,
    )
}

pub fn session_document_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            // String-backed per-session registry key. The DSL declares it as a
            // newtype (`SessionId(pub String)`) so the `Map` key satisfies
            // `Ord + Hash + Clone`; the model domain is the string identity.
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string_enum(
                "SessionFirstTurnPhase",
                &["Inactive", "Pending", "Consumed"],
            ),
            NamedTypeBinding::string_enum("SessionInitialPromptStageDecision", &["Clear", "Store"]),
            NamedTypeBinding::string_enum(
                "SystemContextAppendDecision",
                &["Staged", "Duplicate", "RejectEmpty", "RejectConflict"],
            ),
            NamedTypeBinding::string_enum("SystemContextSource", &["Normal", "RuntimeSteer"]),
            // Realtime-transcript region typed vocabulary (folded from the
            // retired SessionRealtimeTranscriptAuthorityMachine).
            NamedTypeBinding::string_enum("RealtimeTranscriptRoleKind", &["User", "Assistant"]),
            NamedTypeBinding::string_enum("RealtimeTranscriptLaneKind", &["Display", "Spoken"]),
            NamedTypeBinding::string_enum(
                "RealtimeTranscriptStopReasonKind",
                &["Cancelled", "ToolUse", "Other"],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeTranscriptMaterializeDecision",
                &[
                    "Wait",
                    "MarkSkipped",
                    "MaterializeUser",
                    "MaterializeAssistant",
                ],
            ),
            // Durable-config region typed vocabulary (folded from the retired
            // SessionDurableConfigAuthorityMachine).
            NamedTypeBinding::string_enum(
                "SessionDurableProviderKind",
                &["Anthropic", "OpenAI", "Gemini", "SelfHosted", "Other"],
            ),
            NamedTypeBinding::string_enum(
                "SessionToolCategoryOverrideKind",
                &["Inherit", "Enable", "Disable"],
            ),
            NamedTypeBinding::string_enum(
                "SessionCallTimeoutOverrideKind",
                &["Inherit", "Disabled", "Value"],
            ),
            NamedTypeBinding::string_enum(
                "SessionSystemPromptSource",
                &[
                    "DirectMutation",
                    "ExplicitBuild",
                    "DefaultBuild",
                    "WasmDefaultBuild",
                    "RuntimeContextAppend",
                    "RuntimeSteerCleanup",
                ],
            ),
            // Pending-continuation region typed vocabulary (folded from the
            // retired non-canonical PendingContinuationAdmissionMachine).
            NamedTypeBinding::string_enum(
                "ObservedSessionTailKind",
                &[
                    "Empty",
                    "System",
                    "SystemNotice",
                    "User",
                    "Assistant",
                    "BlockAssistant",
                    "ToolResults",
                ],
            ),
            NamedTypeBinding::string_enum(
                "PendingContinuationDisposition",
                &["RunPending", "NoPendingBoundary"],
            ),
            NamedTypeBinding::string_enum(
                "PendingContinuationPublicTerminal",
                &["NoPendingBoundary"],
            ),
            // Resume-override-admission region typed vocabulary (folded from the
            // handwritten session_recovery.rs resolve_effective_turn_config /
            // resolve_resume_llm_binding shell helpers).
            NamedTypeBinding::string_enum(
                "ResumeOverrideRejection",
                &[
                    "ProviderRequiresModel",
                    "ClearAndSetProviderParams",
                    "ClearAndSetAuthBinding",
                    "BuildOnlyAfterFirstTurn",
                ],
            ),
            NamedTypeBinding::string_enum(
                "ResumeProviderSelection",
                &["RecomputeFromModel", "UseOverride", "UseStored"],
            ),
            NamedTypeBinding::string_enum("ResumeSelfHostedSelection", &["Clear", "Retain"]),
        ],
        Vec::new(),
    )
}

/// Canonical SessionTurnAdmissionMachine — the live ephemeral turn-admission
/// gate (`EphemeralSessionService` turn-admission slot). Multi-phase admission
/// lifecycle with a `ShuttingDown` terminal.
pub fn dsl_session_turn_admission_machine() -> MachineSchema {
    session_turn_admission_schema_metadata()
        .attach_to(session_turn_admission::SessionTurnAdmissionMachineState::schema())
}

pub fn dsl_session_turn_admission_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_session_turn_admission_machine(),
        SESSION_TURN_ADMISSION_PRODUCTION_RUST_CRATE,
        SESSION_TURN_ADMISSION_PRODUCTION_RUST_MODULE,
    )
}

pub fn session_turn_admission_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            // The phase enum is referenced as a typed field on the
            // `TurnAdmissionProjected` effect, so it needs a named-type binding.
            NamedTypeBinding::string_enum(
                "TurnAdmissionPhase",
                &["Idle", "Admitted", "Running", "Completing", "ShuttingDown"],
            ),
            NamedTypeBinding::string_enum(
                "StartTurnExecutionKind",
                &["ContentTurn", "ResumePending"],
            ),
            NamedTypeBinding::string_enum(
                "StartTurnDisposition",
                &["RunContentTurn", "RunPending", "NoPendingBoundary"],
            ),
            NamedTypeBinding::string_enum("StartTurnPublicTerminal", &["NoPendingBoundary"]),
            NamedTypeBinding::string_enum(
                "StartTurnDispatchAuthorization",
                &["Authorized", "Cancelled"],
            ),
            // Pending-continuation disposition is owned by the canonical
            // SessionDocumentMachine and consumed here as a typed input. The
            // meerkat-session shell drives SessionDocumentMachine first and
            // mirrors the disposition into this machine's input.
            NamedTypeBinding::string_enum(
                "PendingContinuationDisposition",
                &["RunPending", "NoPendingBoundary"],
            ),
        ],
        Vec::new(),
    )
}

/// Non-canonical support schema used only to emit generated session
/// persistence-version authority into `meerkat-core`.
pub fn dsl_session_persistence_version_authority_machine() -> MachineSchema {
    session_persistence_version_authority_schema_metadata().attach_to(
        session_persistence_version_authority::SessionPersistenceVersionAuthorityMachineState::schema(
        ),
    )
}

pub fn dsl_session_persistence_version_authority_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_session_persistence_version_authority_machine(),
        SESSION_PERSISTENCE_VERSION_AUTHORITY_PRODUCTION_RUST_CRATE,
        SESSION_PERSISTENCE_VERSION_AUTHORITY_PRODUCTION_RUST_MODULE,
    )
}

pub fn session_persistence_version_authority_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![NamedTypeBinding::string_enum(
            "SessionPersistenceVersionField",
            &[
                "SessionEnvelope",
                "StoredInputState",
                "SessionMetadataSchema",
            ],
        )],
        Vec::new(),
    )
}

pub fn dsl_meerkat_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_meerkat_machine(),
        MEERKAT_MACHINE_PRODUCTION_RUST_CRATE,
        MEERKAT_MACHINE_PRODUCTION_RUST_MODULE,
    )
}

pub fn meerkat_machine_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::u64("BoundarySequence"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("RuntimeEpochId"),
            NamedTypeBinding::string("CommsRuntimeId"),
            NamedTypeBinding::string("AuthBindingRef"),
            NamedTypeBinding::string_enum(
                meerkat_core::turn_execution_authority::ContentShape::SCHEMA_TYPE_NAME,
                &meerkat_core::turn_execution_authority::ContentShape::SCHEMA_VARIANTS,
            ),
            NamedTypeBinding::string_enum(
                "DrainExitReason",
                &[
                    "IdleTimeout",
                    "Dismissed",
                    "Failed",
                    "Aborted",
                    "SessionShutdown",
                ],
            ),
            NamedTypeBinding::string_enum(
                "DrainMode",
                &["Timed", "AttachedSession", "PersistentHost"],
            ),
            NamedTypeBinding::string_enum(
                "DrainPhase",
                &["Inactive", "Running", "Stopped", "ExitedRespawnable"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceBaseState",
                &["Absent", "Active", "Removing", "Removed"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceDeltaOperation",
                &["None", "Add", "Remove", "Reload"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceDeltaPhase",
                &["None", "Pending", "Applied", "Draining", "Failed", "Forced"],
            ),
            NamedTypeBinding::string_enum(
                "ExternalToolSurfaceFailureCause",
                &["PendingFailed", "SurfaceDraining", "SurfaceUnavailable"],
            ),
            NamedTypeBinding::string_enum("InboundPeerRequestState", &["Received", "Replied"]),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string_enum(
                "AdmissionInputKind",
                &[
                    "Prompt",
                    "PeerMessage",
                    "PeerRequest",
                    "PeerResponseProgress",
                    "PeerResponseTerminal",
                    "FlowStep",
                    "ExternalEvent",
                    "Continuation",
                    "Operation",
                ],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionContinuationKind",
                &["Ordinary", "WorkgraphAttention"],
            ),
            NamedTypeBinding::string_enum(
                "InputDurabilityKind",
                &["Durable", "Ephemeral", "Derived", "Missing"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionInputOriginKind",
                &["Operator", "Peer", "Flow", "System", "External"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionPolicyApplyMode",
                &["StageRunStart", "StageRunBoundary", "InjectNow", "Ignore"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionPolicyWakeMode",
                &["WakeIfIdle", "InterruptYielding", "None"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionPolicyQueueMode",
                &["None", "Fifo", "Coalesce", "Supersede", "Priority"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionPolicyConsumePoint",
                &[
                    "OnAccept",
                    "OnApply",
                    "OnRunStart",
                    "OnRunComplete",
                    "ExplicitAck",
                ],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionPolicyDrainPolicy",
                &["QueueNextTurn", "SteerBatch", "Immediate", "Ignore"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionRoutingDisposition",
                &["Queue", "Steer", "Immediate", "Drop"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionRunApplyBoundary",
                &["RunStart", "RunCheckpoint", "Immediate"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionRuntimeExecutionKind",
                &["ContentTurn", "ResumePending"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionPeerResponseTerminalApplyIntent",
                &["AppendContextAndRun"],
            ),
            NamedTypeBinding::string_enum("AdmissionPlanKind", &["ConsumedOnAccept", "Queued"]),
            NamedTypeBinding::string_enum(
                "AdmissionQueueActionKind",
                &["None", "EnqueueTo", "EnqueueFront"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionExistingQueuedActionKind",
                &["None", "Coalesce", "Supersede"],
            ),
            NamedTypeBinding::string_enum("AdmissionValidationResultKind", &["Accept", "Reject"]),
            NamedTypeBinding::string_enum(
                "PeerResponseTerminalObservedStatus",
                &["NotPeerTerminal", "Completed", "Failed", "Cancelled"],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionRejectReasonKind",
                &[
                    "DurabilityViolation",
                    "PeerHandlingModeInvalid",
                    "PeerResponseTerminalInvalid",
                ],
            ),
            NamedTypeBinding::string_enum(
                "AdmissionIdempotencyResultKind",
                &["Accept", "Deduplicated"],
            ),
            NamedTypeBinding::string_enum(
                "OpRegistrationAdmissionResultKind",
                &["Accept", "Reject"],
            ),
            NamedTypeBinding::string_enum(
                "OpRegistrationRejectReasonKind",
                &["AlreadyRegistered", "MaxConcurrentExceeded"],
            ),
            NamedTypeBinding::string_enum(
                "OperationPublicResultClass",
                &[
                    "MissingAuthority",
                    "Running",
                    "Completed",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum("OperationCompletionFeedClass", &["Emit", "Suppress"]),
            NamedTypeBinding::string_enum("OperationCompletionWakeClass", &["Wake", "Ignore"]),
            NamedTypeBinding::string_enum("OperationDurabilityClass", &["Retain", "Discard"]),
            NamedTypeBinding::string_enum(
                "OpLifecycleActionKind",
                &[
                    "Start",
                    "Fail",
                    "PeerReady",
                    "ProgressReported",
                    "Complete",
                    "Abort",
                    "Cancel",
                    "RetireRequested",
                    "RetireCompleted",
                    "Terminate",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OpLifecycleRejectReasonKind",
                &[
                    "OperationNotFound",
                    "InvalidTransition",
                    "PeerNotExpected",
                    "AlreadyPeerReady",
                ],
            ),
            NamedTypeBinding::string_enum("WaitAllAdmissionResultKind", &["Accept", "Reject"]),
            NamedTypeBinding::string_enum(
                "WaitAllRejectReasonKind",
                &[
                    "DuplicateOperation",
                    "WaitAlreadyActive",
                    "OperationNotFound",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredInputObservedPhase",
                &[
                    "Accepted",
                    "Queued",
                    "Staged",
                    "Applied",
                    "AppliedPendingConsumption",
                    "Consumed",
                    "Superseded",
                    "Coalesced",
                    "Abandoned",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredInputNormalizationReasonKind",
                &[
                    "QueueAccepted",
                    "RollbackStaged",
                    "BoundaryReceiptCommitted",
                    "MissingBoundaryReceipt",
                ],
            ),
            NamedTypeBinding::string_enum(
                "InputAbandonReason",
                &[
                    "Retired",
                    "Reset",
                    "Stopped",
                    "Destroyed",
                    "Cancelled",
                    "MaxAttemptsExhausted",
                ],
            ),
            NamedTypeBinding::string_enum("InputLane", &["Queue", "Steer"]),
            NamedTypeBinding::string_enum(
                "InputPhase",
                &[
                    "Queued",
                    "Staged",
                    "Applied",
                    "AppliedPendingConsumption",
                    "Consumed",
                    "Superseded",
                    "Coalesced",
                    "Abandoned",
                ],
            ),
            NamedTypeBinding::string_enum(
                "InputTerminalKind",
                &["Consumed", "Superseded", "Coalesced", "Abandoned"],
            ),
            NamedTypeBinding::string_enum(
                "InputPublicLifecycleState",
                &[
                    "Accepted",
                    "Queued",
                    "Staged",
                    "Applied",
                    "AppliedPendingConsumption",
                    "Consumed",
                    "Superseded",
                    "Coalesced",
                    "Abandoned",
                ],
            ),
            NamedTypeBinding::string_enum(
                "InputPublicTerminalOutcome",
                &[
                    "Completed",
                    "Abandoned",
                    "Superseded",
                    "Coalesced",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "InteractionStreamState",
                &[
                    "Reserved",
                    "Attached",
                    "Completed",
                    "Expired",
                    "ClosedEarly",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LlmRetryFailureKind",
                &[
                    "RateLimited",
                    "NetworkTimeout",
                    "CallTimeout",
                    "RetryableProviderError",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveOpenAdmissionRejection",
                &["AlreadyBound", "ChannelAlreadyBound"],
            ),
            NamedTypeBinding::string_enum("LiveRefreshPublicStatus", &["Queued"]),
            NamedTypeBinding::string_enum("LiveClosePublicStatus", &["Closed"]),
            NamedTypeBinding::string_enum(
                "LiveCommandPublicKind",
                &[
                    "SendInput",
                    "CommitInput",
                    "Interrupt",
                    "TruncateAssistantOutput",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveCommandRejectionReason",
                &[
                    "ChannelNotFound",
                    "NoAdapter",
                    "ChannelNotReady",
                    "UnsupportedCommand",
                    "AdapterError",
                    "InternalHostError",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveCommandRejectionPublicErrorClass",
                &["InvalidParams", "InternalError"],
            ),
            NamedTypeBinding::string_enum(
                "LiveChannelRequestPublicKind",
                &["Status", "Close", "Refresh", "WebrtcAnswer"],
            ),
            NamedTypeBinding::string_enum(
                "LiveChannelRequestRejectionReason",
                &[
                    "ChannelNotFound",
                    "NoAdapter",
                    "InvalidToken",
                    "InvalidPayload",
                    "WebrtcAnswerError",
                    "InternalHostError",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveChannelRequestRejectionPublicErrorClass",
                &["InvalidParams", "InternalError"],
            ),
            NamedTypeBinding::string_enum(
                "LiveWebrtcAnswerAdmissionRejection",
                &[
                    "TokenNotFound",
                    "TokenExpired",
                    "TokenChannelMismatch",
                    "TokenAlreadyConsumed",
                    "ChannelNotBound",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveWebsocketTokenAdmissionRejection",
                &[
                    "TokenNotFound",
                    "TokenExpired",
                    "TokenChannelMismatch",
                    "TokenAlreadyConsumed",
                    "ChannelNotBound",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LiveWebsocketTokenAdmissionPublicErrorClass",
                &["InvalidToken"],
            ),
            NamedTypeBinding::string_enum("LiveWebrtcAnswerPublicStatus", &["Answered"]),
            NamedTypeBinding::string_enum(
                "RpcEventStreamTerminalReason",
                &["RemoteEnd", "TerminalError", "ExplicitClose"],
            ),
            NamedTypeBinding::string_enum(
                "RpcEventStreamTerminalObservationKind",
                &[
                    "TransportEnded",
                    "NotificationQueueOverflow",
                    "NotificationReceiverGone",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RpcEventStreamTerminalErrorCode",
                &["StreamQueueOverflow", "StreamReceiverGone"],
            ),
            NamedTypeBinding::string_enum(
                "LiveChannelPublicStatus",
                &["Idle", "Opening", "Ready", "Degraded", "Closing", "Closed"],
            ),
            NamedTypeBinding::string_enum(
                "LiveChannelDegradationReason",
                &[
                    "Unknown",
                    "RateLimited",
                    "ProviderThrottled",
                    "NetworkUnstable",
                    "Other",
                ],
            ),
            NamedTypeBinding::string("McpServerId"),
            NamedTypeBinding::string_enum(
                "McpServerState",
                &["PendingConnect", "Connected", "Failed", "Disconnected"],
            ),
            NamedTypeBinding::string("MeerkatPhase"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("OperationId"),
            NamedTypeBinding::string_enum(
                "OperationKind",
                &[
                    "MobMemberChild",
                    "BackgroundToolOp",
                    "BackgroundToolCapacitySlot",
                ],
            ),
            NamedTypeBinding::string_enum("OperationSourceKind", &["SessionChild", "BackendPeer"]),
            NamedTypeBinding::type_path_struct(
                "OperationSource",
                "crate::catalog::dsl::meerkat_machine::OperationSource",
                vec![
                    TypePathStructField::named("kind", "OperationSourceKind"),
                    TypePathStructField::optional_named("session_id", "SessionId"),
                    TypePathStructField::optional_named("peer_id", "PeerId"),
                    TypePathStructField::optional_named("address", "PeerAddress"),
                ],
            ),
            NamedTypeBinding::string_enum(
                "OperationStatus",
                &[
                    "Absent",
                    "Provisioning",
                    "Running",
                    "Retiring",
                    "Completed",
                    "Failed",
                    "Aborted",
                    "Cancelled",
                    "Retired",
                    "Terminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OperationTerminalOutcomeKind",
                &[
                    "Completed",
                    "Failed",
                    "Aborted",
                    "Cancelled",
                    "Retired",
                    "Terminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OutboundPeerRequestState",
                &[
                    "Sent",
                    "AcceptedProgress",
                    "Completed",
                    "Failed",
                    "TimedOut",
                ],
            ),
            NamedTypeBinding::string("PeerCorrelationId"),
            NamedTypeBinding::string_enum(
                "PeerIngressAdmittedKind",
                &["Message", "Request", "Response", "Ack", "PlainEvent"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressAuthClass",
                &["Required", "SupervisorBridgeExempt"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressEnvelopeClass",
                &["Message", "Request", "Lifecycle", "Response", "Ack"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressInputClass",
                &[
                    "ActionableMessage",
                    "ActionableRequest",
                    "ResponseProgress",
                    "ResponseTerminal",
                    "PeerLifecycleAdded",
                    "PeerLifecycleRetired",
                    "PeerLifecycleUnwired",
                    "SilentRequest",
                    "Ack",
                    "PlainEvent",
                ],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressLifecycleClass",
                &["PeerAdded", "PeerRetired", "PeerUnwired"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressAuthorityPhaseClass",
                &["Absent", "Received", "Dropped", "Delivered"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressReceiveOutcomeClass",
                &[
                    "Admitted",
                    "DroppedUntrustedSender",
                    "DroppedSessionClosed",
                    "DroppedInboxFull",
                ],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressAdmissionDiagnosticClass",
                &["TrustedAtAdmission", "UntrustedAtAdmission"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressOwnerKind",
                &["Unattached", "SessionOwned", "MobOwned"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressResponseStatus",
                &["Accepted", "Completed", "Failed"],
            ),
            NamedTypeBinding::string_enum(
                "PeerIngressResponseTerminality",
                &["Progress", "TerminalCompleted", "TerminalFailed"],
            ),
            NamedTypeBinding::string_enum("PeerTerminalDisposition", &["Completed", "Failed"]),
            NamedTypeBinding::string_enum(
                "PostAdmissionSignalKind",
                &[
                    "WakeLoop",
                    "InterruptYielding",
                    "RequestImmediateProcessing",
                ],
            ),
            NamedTypeBinding::string_enum("PreRunPhase", &["Idle", "Attached", "Retired"]),
            NamedTypeBinding::string_enum(
                "StagedSessionPhase",
                &["NotStaged", "Staged", "Promoting", "Closing"],
            ),
            NamedTypeBinding::string_enum(
                "MobOperatorAccessRequestKind",
                &["Inherit", "Enable", "Disable"],
            ),
            NamedTypeBinding::type_path(
                "MobToolCallerProvenance",
                "meerkat_core::service::MobToolCallerProvenance",
            ),
            NamedTypeBinding::type_path(
                "OpaquePrincipalToken",
                "meerkat_core::service::OpaquePrincipalToken",
            ),
            NamedTypeBinding::string_enum(
                "Provider",
                &["Anthropic", "OpenAI", "Gemini", "SelfHosted", "Other"],
            ),
            NamedTypeBinding::string_enum("RegistrationPhase", &["Queuing", "Active"]),
            NamedTypeBinding::string_enum(
                "RoutingApprovalParentKind",
                &["SwitchTurn", "ImageOperation"],
            ),
            NamedTypeBinding::string_enum(
                "RoutingApprovalPhase",
                &[
                    "Pending",
                    "PresentedToUser",
                    "Approved",
                    "Denied",
                    "SurfaceDetached",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingDenialReason",
                &[
                    "CapabilityPolicy",
                    "ApprovalRequiredButUnavailable",
                    "DeniedDuringApproval",
                    "ScopedOverrideConflict",
                    "RealtimeTransportConflict",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingSwitchApprovalReason",
                &[
                    "CrossProvider",
                    "CostExceedsThreshold",
                    "SafetyHold",
                    "UntilChangedFromModelOrigin",
                    "RealtimeDetachRequired",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageApprovalReason",
                &[
                    "CrossProvider",
                    "CostExceedsThreshold",
                    "SafetyHold",
                    "RealtimeDetachRequired",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImagePlanDenialReason",
                &[
                    "UnsupportedTarget",
                    "UnsupportedCount",
                    "CapabilityPolicy",
                    "CostPolicy",
                    "SafetyPolicy",
                    "ApprovalRequiredButUnavailable",
                    "DeniedDuringApproval",
                    "ScopedOverrideConflict",
                    "RealtimeTransportConflict",
                    "ProjectionUnsupported",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageOperationPhase",
                &[
                    "Requested",
                    "PlanResolved",
                    "ScopedOverrideActive",
                    "ProviderCallInFlight",
                    "ResultCommitted",
                    "RestoringScopedOverride",
                    "Terminal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageTerminal",
                &[
                    "Generated",
                    "Denied",
                    "EmptyResult",
                    "RefusedByProvider",
                    "SafetyFiltered",
                    "Failed",
                    "Cancelled",
                    "Timeout",
                    "ScopedRestoreFailed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageTerminalObservation",
                &[
                    "Generated",
                    "EmptyResult",
                    "ProviderHttpError",
                    "ProviderNativeError",
                    "ExecutionFailed",
                    "BlobCommitFailed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingImageProviderErrorCode",
                &[
                    "Unknown",
                    "OpenAiContentFilter",
                    "OpenAiModelRefusal",
                    "GeminiSafety",
                    "GeminiModelRefusal",
                    "GeminiDeadlineExceeded",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingProviderTextDisposition",
                &["NotEmitted", "Captured", "EmittedButNotStored"],
            ),
            NamedTypeBinding::string_enum("MobPeerOverlayCommandKind", &["Wire", "Unwire"]),
            NamedTypeBinding::string_enum(
                "SupervisorBridgeCommandAdmissionResultKind",
                &["Accept", "Reject"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBridgeCommandRejectionKind",
                &["NotBound", "StaleSupervisor", "SenderMismatch"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBindAdmissionResultKind",
                &["Bootstrap", "IdempotentAck", "Reject"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBindRejectionKind",
                &["AlreadyBound", "SenderMismatch"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBindMaterialAdmissionKind",
                &[
                    "Accept",
                    "AddressMismatch",
                    "SenderMismatch",
                    "InvalidPeerSpec",
                    "InvalidBootstrapToken",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TranscriptEditAdmissionKind",
                &["Admissible", "DeniedBusy"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorAuthorizeAdmissionResultKind",
                &["Proceed", "IdempotentAck", "Reject"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorAuthorizeRejectionKind",
                &["NotBound", "StaleSupervisor", "SenderMismatch"],
            ),
            NamedTypeBinding::string_enum(
                "RoutingSwitchTurnPhase",
                &[
                    "Requested",
                    "PendingForBoundary",
                    "ActiveFiniteOverride",
                    "ApplyingPersistentReconfigure",
                    "Terminal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RoutingSwitchTurnTerminal",
                &[
                    "Denied",
                    "ConsumedAndRestored",
                    "PersistentReconfigureApplied",
                ],
            ),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string_enum(
                "RunFailureSourceKind",
                &[
                    "Unknown",
                    "Llm",
                    "StoreError",
                    "ToolError",
                    "McpError",
                    "SessionNotFound",
                    "TokenBudgetExceeded",
                    "TimeBudgetExceeded",
                    "ToolCallBudgetExceeded",
                    "MaxTokensReached",
                    "ContentFiltered",
                    "MaxTurnsReached",
                    "Cancelled",
                    "InvalidStateTransition",
                    "OperationNotFound",
                    "DepthLimitExceeded",
                    "ConcurrencyLimitExceeded",
                    "ConfigError",
                    "InvalidToolAccess",
                    "InternalError",
                    "BuildError",
                    "AuthReauthRequired",
                    "CallbackPending",
                    "StructuredOutputValidationFailed",
                    "InvalidOutputSchema",
                    "HookDenied",
                    "HookTimeout",
                    "HookExecutionFailed",
                    "HookConfigInvalid",
                    "TerminalFailure",
                    "NoPendingBoundary",
                    "LlmRetryExhausted",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeApplyFailureCause",
                &[
                    "Unknown",
                    "PrimitiveRejected",
                    "RuntimeContextApply",
                    "RuntimeTurn",
                    "HookDenied",
                    "HookRuntimeFailure",
                    "ExecutorStopped",
                    "ExecutorControlFailed",
                    "ExecutorInternal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeNoticeKind",
                &["Drain", "Reset", "Stop", "Exit", "Recover"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeEffectKind",
                &["CancelAfterBoundary", "StopRuntimeExecutor"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionObservedOutcome",
                &[
                    "Completed",
                    "CompletedWithoutResult",
                    "CallbackPending",
                    "Cancelled",
                    "Abandoned",
                    "RuntimeApplyFailed",
                    "FinalizationFailed",
                    "RuntimeTerminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionTerminalObservation",
                &[
                    "RunResult",
                    "NoResult",
                    "CallbackPending",
                    "MachineTerminal",
                    "RuntimeTerminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionFinalizationObservation",
                &["Succeeded", "Failed"],
            ),
            NamedTypeBinding::string_enum(
                "UserInterruptObservationKind",
                &[
                    "Accepted",
                    "IdleNoop",
                    "AttachedNoop",
                    "StagedNoop",
                    "Destroyed",
                    "NotInterruptible",
                ],
            ),
            NamedTypeBinding::string_enum(
                "UserInterruptPublicResultKind",
                &["Interrupted", "NotFound", "SessionBusy", "Conflict"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionResultClass",
                &[
                    "Completed",
                    "CompletedWithoutResult",
                    "CallbackPending",
                    "Cancelled",
                    "AbandonedWithError",
                    "CompletedWithFinalizationFailure",
                    "RuntimeTerminated",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionLiveSessionObservation",
                &["NotObserved", "Present", "Absent"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionCleanupAction",
                &["RetainRuntime", "CleanupRuntime"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionPreAdmissionAction",
                &["RetainPreAdmission", "ReleasePreAdmission"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionWaitFailureObservation",
                &["ChannelClosed", "AuthorityUnavailable"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionWaitFailurePublicErrorClass",
                &["InternalError"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionWaitFailurePublicReason",
                &["CompletionChannelClosed", "CompletionAuthorityUnavailable"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeOpsLifecycleDurabilityAction",
                &["RetainSnapshot", "DeleteSnapshot"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeLifecycleObservedState",
                &[
                    "Initializing",
                    "Idle",
                    "Attached",
                    "Running",
                    "Retired",
                    "Stopped",
                    "Destroyed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeLifecycleTerminality",
                &["NonTerminal", "Terminal"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeInputAdmission",
                &["RejectsInput", "AcceptsInput"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeQueueAdmission",
                &["BlocksQueue", "ProcessesQueue"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimePrepareAdmission",
                &["NotReady", "Ready", "Destroyed"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeIngressAdmission",
                &["Open", "NotReady", "Destroyed"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeLoopRunBinding",
                &["Blocked", "AllocateNew", "UsePrebound"],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredInputKind",
                &[
                    "Prompt",
                    "PeerMessage",
                    "PeerRequest",
                    "PeerResponseProgress",
                    "PeerResponseTerminal",
                    "FlowStep",
                    "ExternalEvent",
                    "Continuation",
                    "Operation",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredInputRecoveryDisposition",
                &["Retain", "Discard"],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredRunApplyBoundary",
                &["RunStart", "RunCheckpoint", "Immediate"],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredRuntimeExecutionKind",
                &["ContentTurn", "ResumePending"],
            ),
            NamedTypeBinding::string_enum(
                "RecoveredPeerResponseTerminalApplyIntent",
                &["AppendContextAndRun"],
            ),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("SessionLlmCapabilitySurface"),
            NamedTypeBinding::string_enum(
                "SessionLlmCapabilitySurfaceStatus",
                &["Unresolved", "Resolved"],
            ),
            NamedTypeBinding::string("SessionLlmIdentity"),
            NamedTypeBinding::string("SessionToolVisibilityDelta"),
            NamedTypeBinding::string("SessionToolVisibilityState"),
            NamedTypeBinding::string_enum("SupervisorBindingKind", &["Unbound", "Bound"]),
            NamedTypeBinding::string("SurfaceDeltaOperation"),
            NamedTypeBinding::string("SurfaceDeltaPhase"),
            NamedTypeBinding::string_enum("SurfacePhase", &["Operating", "Shutdown"]),
            NamedTypeBinding::string("SurfaceId"),
            NamedTypeBinding::string_enum("SurfacePendingOp", &["None", "Add", "Reload"]),
            NamedTypeBinding::string_enum(
                "SurfaceRequestPhase",
                &["Pending", "Published", "Cancelled", "Completed"],
            ),
            NamedTypeBinding::string_enum(
                "SurfaceRequestTerminalPolicy",
                &["RespondWithoutPublish", "PublishOnSuccess"],
            ),
            NamedTypeBinding::string_enum("SurfaceStagedOp", &["None", "Add", "Remove", "Reload"]),
            NamedTypeBinding::type_path_enum_with_structural_variants(
                "ToolFilter",
                "crate::catalog::dsl::meerkat_machine::ToolFilter",
                &["All"],
                vec![
                    TypePathEnumStructuralVariant::string_set("Allow", "names"),
                    TypePathEnumStructuralVariant::string_set("Deny", "names"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "ToolProvenance",
                "crate::catalog::dsl::meerkat_machine::ToolProvenance",
                vec![
                    TypePathStructField::named("kind", "ToolSourceKind"),
                    TypePathStructField::string("source_id"),
                ],
            ),
            NamedTypeBinding::string_enum(
                "ToolSourceKind",
                &[
                    "Builtin",
                    "Shell",
                    "Comms",
                    "Memory",
                    "Schedule",
                    "WorkGraph",
                    "Mob",
                    "Callback",
                    "Mcp",
                    "RustBundle",
                ],
            ),
            NamedTypeBinding::string_enum("TurnCancellationReason", &["Observed"]),
            NamedTypeBinding::type_path_field_presence_set(
                "ToolVisibilityWitness",
                "crate::catalog::dsl::meerkat_machine::ToolVisibilityWitness",
                &["stable_owner_key", "last_seen_provenance"],
            ),
            NamedTypeBinding::string_enum(
                "TurnPhase",
                &[
                    "Ready",
                    "ApplyingPrimitive",
                    "CallingLlm",
                    "WaitingForOps",
                    "DrainingBoundary",
                    "Extracting",
                    "ErrorRecovery",
                    "Cancelling",
                    "Completed",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TurnPrimitiveKind",
                &[
                    "None",
                    "ConversationTurn",
                    "ImmediateAppend",
                    "ImmediateContextAppend",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TurnTerminalOutcome",
                &[
                    "None",
                    "Completed",
                    "Failed",
                    "Cancelled",
                    "BudgetExhausted",
                    "TimeBudgetExceeded",
                    "StructuredOutputValidationFailed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TurnTerminalCauseKind",
                &[
                    "Unknown",
                    "HookDenied",
                    "HookFailure",
                    "LlmFailure",
                    "ToolFailure",
                    "StructuredOutputValidationFailed",
                    "BudgetExhausted",
                    "TimeBudgetExceeded",
                    "RetryExhausted",
                    "TurnLimitReached",
                    "RuntimeApplyFailure",
                    "FatalFailure",
                ],
            ),
            NamedTypeBinding::string_enum(
                "TerminalCauseClass",
                &[
                    "Missing",
                    "Unknown",
                    "BudgetExhausted",
                    "TimeBudgetExceeded",
                    "RetryExhausted",
                    "StructuredOutputValidationFailed",
                    "OtherFailure",
                ],
            ),
            NamedTypeBinding::string_enum(
                "SurfaceResultClass",
                &["Success", "HardFailure", "Cancelled", "MissingTerminal"],
            ),
            NamedTypeBinding::string_enum(
                "LlmFailureRecoveryKind",
                &["Fatal", "Recover", "Exhausted"],
            ),
            NamedTypeBinding::u64("TurnNumber"),
            NamedTypeBinding::string("WaitRequestId"),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::string_enum("WorkOrigin", &["External", "Internal", "Ingest"]),
            // Wave-c C-6r: typed PeerEndpoint twin.
            NamedTypeBinding::type_path_struct(
                "PeerEndpoint",
                "crate::catalog::dsl::meerkat_machine::PeerEndpoint",
                vec![
                    TypePathStructField::named("name", "PeerName"),
                    TypePathStructField::named("peer_id", "PeerId"),
                    TypePathStructField::named("address", "PeerAddress"),
                    TypePathStructField::named("signing_key", "PeerSigningKey"),
                ],
            ),
            NamedTypeBinding::type_path(
                "PeerName",
                "crate::catalog::dsl::meerkat_machine::PeerName",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::meerkat_machine::PeerId"),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::meerkat_machine::PeerAddress",
            ),
            NamedTypeBinding::type_path(
                "PeerSigningKey",
                "crate::catalog::dsl::meerkat_machine::PeerSigningKey",
            ),
        ],
        input_variant_ids(MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS),
    )
    .with_ci_step_limit(1)
}

runtime_internal_inputs!(
    MeerkatMachineRuntimeInternalInput,
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS,
    meerkat_machine::MeerkatMachineInputVariant,
    [
        AbandonInput,
        AbandonLiveOpenAdmission,
        AbortOp,
        AcknowledgeTerminal,
        AddDirectPeerEndpoint,
        AdvanceSessionContext,
        ApplyMobPeerOverlay,
        AttachMobIngress,
        AttachSessionIngress,
        AuthorizeSupervisor,
        AuthorizeDeferredSessionSystemContextAppend,
        BindSupervisor,
        BoundaryComplete,
        BoundaryContinue,
        BudgetExhausted,
        CancelNow,
        CancelOp,
        CancelRun,
        CancelWaitAll,
        CancellationObserved,
        ChangeLane,
        ClearLocalEndpoint,
        CoalesceInput,
        CommitDeferredNames,
        CommitVisibilityFilter,
        CompleteOp,
        CompleteUntilChangedSwitchTurnReconfigure,
        ConsumeInput,
        ConsumeOnAccept,
        DetachIngress,
        EnterExtraction,
        ExtractionFailed,
        ExtractionStart,
        ExtractionValidationFailed,
        ExtractionValidationPassed,
        FailOp,
        FatalFailure,
        ForceCancelNoRun,
        IncrementAttemptCount,
        InterruptCurrentRun,
        InteractionStreamAttached,
        InteractionStreamClosedEarly,
        InteractionStreamCompleted,
        InteractionStreamExpired,
        InteractionStreamReserved,
        LlmReturnedTerminal,
        LlmReturnedToolCalls,
        MarkApplied,
        MarkAppliedPendingConsumption,
        McpServerConnectPending,
        McpServerConnected,
        McpServerDisconnected,
        McpServerFailed,
        McpServerReload,
        ModelRoutingStatus,
        OpsBarrierSatisfied,
        PeerReadyOp,
        PeerRequestReceived,
        PeerRequestSent,
        PeerRequestTimedOut,
        PeerResponseProgressArrived,
        PeerResponseReplied,
        PeerResponseTerminalArrived,
        PrimitiveApplied,
        ProgressReportedOp,
        QueueAccepted,
        RecordBoundarySeq,
        RecordLiveChannelStatus,
        RecordLiveChannelRequestRejected,
        RecordLiveCloseClosed,
        RecordLiveCommandAccepted,
        RecordLiveCommandRejected,
        RecordLiveRefreshQueued,
        RecordMobEventStreamOpened,
        RecordMobEventStreamTerminated,
        RecordSessionEventStreamOpened,
        RecordSessionEventStreamTerminated,
        RecoverAdmittedInput,
        PrioritizeInput,
        DeferInputBehindBacklog,
        PublishLocalEndpoint,
        RecoverableFailure,
        RecoverInputLifecycle,
        RecoverRuntimeAuthority,
        RegisterOp,
        RegisterPendingOps,
        RemoveDirectPeerEndpoint,
        RequestCancelAfterBoundary,
        RequestFiniteSwitchTurn,
        RequestUntilChangedSwitchTurn,
        RequestWaitAll,
        ResolveLiveOpenAdmission,
        ResolveMobEventStreamClose,
        ResolveSessionEventStreamClose,
        ResolveStagedRollback,
        RetireCompletedOp,
        RetireRequestedOp,
        RetryRequested,
        RevokeSupervisor,
        RollbackRun,
        RollbackStaged,
        RunCancelled,
        RunCompleted,
        RunFailed,
        RuntimeExecutorExited,
        SatisfyWaitAll,
        SetModelRoutingBaseline,
        SpawnDrain,
        StageDeferredNames,
        StageForRun,
        StageVisibilityFilter,
        StartConversationRun,
        StartImmediateAppend,
        StartImmediateContext,
        StartOp,
        SteerAccepted,
        StopDrain,
        SupersedeInput,
        SurfaceApplyBoundary,
        SurfaceCallFinished,
        SurfaceCallStarted,
        SurfaceFinalizeRemovalClean,
        SurfaceFinalizeRemovalForced,
        SurfaceMarkPendingFailed,
        SurfaceMarkPendingSucceeded,
        SurfaceSnapshotAligned,
        SurfaceShutdown,
        SurfaceRegister,
        SurfaceStageAdd,
        SurfaceStageReload,
        SurfaceStageRemove,
        SyncVisibilityRevisions,
        SupervisorTrustEdgePublishFailed,
        SupervisorTrustEdgePublished,
        SupervisorTrustEdgeRevokeFailed,
        SupervisorTrustEdgeRevoked,
        TerminateOp,
        TimeBudgetExceeded,
        ToolCallsResolved,
        TurnLimitReached,
        AdmitSurfaceRequest,
        AbandonDeferredSessionPromotion,
        AdvanceAgentCompletionCursor,
        AdvanceRuntimeInjectedCompletionCursor,
        AdvanceRuntimeObservedCompletionCursor,
        AuthorizeDeferredSessionMachineArchivedResume,
        AuthorizeStoredInputStateSeed,
        AuthorizeSupervisorMobPeerOverlay,
        BeginDeferredSessionArchive,
        BeginDeferredSessionPromotion,
        CancelSurfaceRequest,
        ClassifyInputTerminality,
        ClassifyOperationTerminality,
        ClassifyOperationPublicResult,
        ClassifyOperationTransitionIdempotence,
        ClassifyLlmFailureRecovery,
        ClassifyOperationCompletionFeed,
        ClassifyOperationCompletionWake,
        ClassifyOperationDurability,
        ClassifyRecoveredInputDurability,
        ClassifyRecoveredOperationRecord,
        ClassifyRuntimeLifecycleDurability,
        ClassifyRuntimeLifecycleState,
        ClassifyRuntimeLoopQueueAdmission,
        ClassifySurfaceRequestTerminal,
        ClassifyTurnTerminality,
        ClassifyTurnTerminalCauseClass,
        ClearSessionLlmState,
        ClearTurnToolOverlay,
        CollectCompletedOp,
        DropDeferredSession,
        EvictCompletedOp,
        FinishDeferredSessionArchive,
        FinishDeferredSessionPromotion,
        FinishSurfaceRequestUnpublished,
        GrantMobOperatorManageMob,
        HydrateSessionLlmState,
        NormalizeRecoveredInputLifecycle,
        PeerResponseRejected,
        PublishOrCancelSurfaceRequest,
        PublishSurfaceRequest,
        RecordLiveWebrtcAnswerAccepted,
        RecordLiveWebrtcTokenIssued,
        RecordLiveWebsocketTokenIssued,
        RecoverCompletionConsumerCursors,
        RecoverCompletionFeedEntry,
        RecoverOpRecord,
        RecoverOpsCompletionCursor,
        RegisterAcceptedIdempotency,
        ReplaceDeferredToolAuthorityCatalog,
        ReplaceFilterToolAuthorityCatalog,
        RequestSupervisorTrustPublish,
        ResolveAdmissionIdempotency,
        ResolveAdmissionPlan,
        ResolveAdmissionValidation,
        ResolveInputPublicLifecycle,
        ResolveInputPublicTerminalOutcome,
        ResolveLiveBoundaryContextReceipt,
        ResolveLiveWebrtcAnswerAdmission,
        ResolveLiveWebsocketTokenAdmission,
        ResolveMobOperatorCreateAuthority,
        ResolveOpLifecycleTransitionRejection,
        ResolvePeerIngressDequeue,
        ResolvePeerIngressReceive,
        ResolveRuntimeCompletionCleanup,
        ResolveRuntimeCompletionResult,
        ResolveRuntimeCompletionWaitFailure,
        ResolveRuntimeOpsLifecycleDurability,
        ResolveSupervisorAuthorizeAdmission,
        ResolveSupervisorBindAdmission,
        ResolveSupervisorBindMaterialAdmission,
        ResolveSupervisorBridgeCommandAdmission,
        ResolveTranscriptEditAdmission,
        ResolveTurnSurfaceResult,
        ResolveUserInterruptPublicResult,
        ResolveWaitAllAdmission,
        RestoreDeferredSessionArchive,
        RestoreMobOperatorAuthority,
        SetMobOperatorCreateAuthority,
        SetMobOperatorProfileMutation,
        SetMobOperatorSpawnProfilesInMob,
        SetTurnToolOverlay,
        StageDeferredSession,
        SurfaceSetRemovalTimeout,
        UpdateDeferredSessionKeepAlive,
        UpdateDeferredSessionLlmIdentity,
    ]
);

pub fn meerkat_machine_runtime_internal_input_variants()
-> Vec<meerkat_machine::MeerkatMachineInputVariant> {
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS
        .iter()
        .copied()
        .map(MeerkatMachineRuntimeInternalInput::input_variant)
        .collect()
}

pub fn dsl_mob_machine() -> MachineSchema {
    mob_machine_schema_metadata().attach_to(mob_machine::MobMachineState::schema())
}

pub fn dsl_mob_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_mob_machine(),
        MOB_MACHINE_PRODUCTION_RUST_CRATE,
        MOB_MACHINE_PRODUCTION_RUST_MODULE,
    )
}

pub fn mob_machine_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string_enum(
                "CancelAllWorkRejectReasonKind",
                &["MobNotRunning", "MemberNotFound", "StaleFenceToken"],
            ),
            NamedTypeBinding::string_enum("CollectionPolicyKind", &["All", "Any", "Quorum"]),
            NamedTypeBinding::string_enum("DependencyMode", &["All", "Any"]),
            NamedTypeBinding::string_enum(
                "EventSubscriptionRejectReasonKind",
                &["MemberNotFound", "NoSessionBinding"],
            ),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::string_enum(
                "FlowFrameReducerCommandKind",
                &[
                    "StartRootFrame",
                    "StartBodyFrame",
                    "AdmitNextReadyNode",
                    "CompleteNode",
                    "RecordNodeOutput",
                    "FailNode",
                    "SkipNode",
                    "CancelNode",
                    "SealFrame",
                ],
            ),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentIdentity"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("BranchId"),
            NamedTypeBinding::type_path_struct(
                "ExternalPeerEdge",
                "crate::catalog::dsl::mob_machine::ExternalPeerEdge",
                vec![
                    TypePathStructField::named("local", "AgentIdentity"),
                    TypePathStructField::named("endpoint", "ExternalPeerEndpoint"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "ExternalPeerEndpoint",
                "crate::catalog::dsl::mob_machine::ExternalPeerEndpoint",
                vec![
                    TypePathStructField::named("name", "PeerName"),
                    TypePathStructField::named("peer_id", "PeerId"),
                    TypePathStructField::named("address", "PeerAddress"),
                    TypePathStructField::named("signing_key", "PeerSigningKey"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "MemberPeerEndpoint",
                "crate::catalog::dsl::mob_machine::MemberPeerEndpoint",
                vec![
                    TypePathStructField::named("name", "PeerName"),
                    TypePathStructField::named("peer_id", "PeerId"),
                    TypePathStructField::named("address", "PeerAddress"),
                    TypePathStructField::named("signing_key", "PeerSigningKey"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "ExternalPeerKey",
                "crate::catalog::dsl::mob_machine::ExternalPeerKey",
                vec![
                    TypePathStructField::named("local", "AgentIdentity"),
                    TypePathStructField::named("name", "PeerName"),
                ],
            ),
            NamedTypeBinding::string("FlowNodeId"),
            NamedTypeBinding::string_enum("FlowNodeKind", &["Step", "Loop"]),
            NamedTypeBinding::string_enum(
                "FlowRunReducerCommandKind",
                &[
                    "CreateRun",
                    "StartRun",
                    "DispatchStep",
                    "CompleteStep",
                    "RecordStepOutput",
                    "ConditionPassed",
                    "ConditionRejected",
                    "FailStep",
                    "SkipStep",
                    "ProjectFrameStepStatus",
                    "CancelStep",
                    "RegisterTargets",
                    "RecordTargetSuccess",
                    "RecordTargetTerminalFailure",
                    "RecordTargetCanceled",
                    "RecordTargetFailure",
                    "RegisterReadyFrame",
                    "PumpNodeScheduler",
                    "RegisterPendingBodyFrame",
                    "PumpFrameScheduler",
                    "NodeExecutionReleased",
                    "FrameTerminated",
                    "TerminalizeCompleted",
                    "TerminalizeFailed",
                    "TerminalizeCanceled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "FlowRunStatus",
                &[
                    "Absent",
                    "Pending",
                    "Running",
                    "Completed",
                    "Failed",
                    "Canceled",
                ],
            ),
            NamedTypeBinding::string_enum("FlowRunPublicResultClassKind", &["Failure", "Success"]),
            NamedTypeBinding::string("FrameId"),
            NamedTypeBinding::string_enum("FrameScope", &["Root", "Body"]),
            NamedTypeBinding::string_enum(
                "FrameStatus",
                &["Running", "Completed", "Failed", "Canceled"],
            ),
            NamedTypeBinding::string_enum(
                "KickoffIntent",
                &[
                    "Pending",
                    "Starting",
                    "Started",
                    "CallbackPending",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "KickoffPhase",
                &[
                    "Pending",
                    "Starting",
                    "CallbackPending",
                    "Started",
                    "Failed",
                    "Cancelled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LoopIterationReducerCommandKind",
                &[
                    "StartLoop",
                    "BodyFrameStarted",
                    "BodyFrameCompleted",
                    "BodyFrameFailed",
                    "BodyFrameCanceled",
                    "UntilConditionMet",
                    "UntilConditionFailed",
                    "CancelLoop",
                ],
            ),
            NamedTypeBinding::string_enum(
                "LoopIterationStage",
                &[
                    "AwaitingBodyFrame",
                    "BodyFrameActive",
                    "AwaitingUntilEvaluation",
                ],
            ),
            NamedTypeBinding::string("LoopId"),
            NamedTypeBinding::string("LoopInstanceId"),
            NamedTypeBinding::string_enum(
                "LoopStatus",
                &["Running", "Completed", "Exhausted", "Failed", "Canceled"],
            ),
            NamedTypeBinding::string_enum(
                "MemberLifecycleKind",
                &[
                    "Spawned",
                    "Retiring",
                    "Retired",
                    "Reset",
                    "Respawned",
                    "Completed",
                    "Destroyed",
                ],
            ),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string_enum(
                "MobLifecycleJournalKind",
                &[
                    "Completed",
                    "Destroying",
                    "DestroyStorageFinalizing",
                    "MemberSpawned",
                    "MemberRetired",
                    "Reset",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MobFlowDelegationEdgeRuleVerdictKind",
                &["Allow", "Deny"],
            ),
            NamedTypeBinding::string_enum("MobFlowDelegationEdgeModeKind", &["Advisory", "Strict"]),
            NamedTypeBinding::string_enum(
                "MobFlowDelegationEdgeAdmissionKind",
                &["Admitted", "DeniedStrict", "DeniedAdvisory"],
            ),
            NamedTypeBinding::string_enum(
                "MobRemoteMemberRuntimeObservedState",
                &[
                    "Initializing",
                    "Idle",
                    "Attached",
                    "Running",
                    "Retired",
                    "Stopped",
                    "Destroyed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MobRemoteMemberRuntimeTerminality",
                &["NonTerminal", "Terminal"],
            ),
            NamedTypeBinding::string_enum("MobSpawnMemberAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum("MobCurrentMobAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum("MobCreateMobAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum(
                "MobProfileMutationAdmissionKind",
                &["Denied", "Allowed"],
            ),
            NamedTypeBinding::string_enum(
                "MobBridgeRejectionCause",
                &[
                    "NotBound",
                    "StaleSupervisor",
                    "SenderMismatch",
                    "AlreadyBound",
                    "InvalidBootstrapToken",
                    "UnsupportedProtocolVersion",
                    "InvalidSupervisorSpec",
                    "InvalidPeerSpec",
                    "AddressMismatch",
                    "Unsupported",
                    "Internal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MobBridgeRejectionRecovery",
                &["FatalBubbleUp", "RebindRecover"],
            ),
            NamedTypeBinding::string_enum(
                "MobPendingSupervisorAcceptanceKind",
                &["Fatal", "NotConfirmedReattempt", "StalePendingAuthority"],
            ),
            NamedTypeBinding::string_enum("MobFrameSeedDisposition", &["Seeded", "AlreadySeeded"]),
            NamedTypeBinding::string_enum(
                "MobSpawnManyFailureObservationKind",
                &[
                    "ProfileNotFound",
                    "MemberNotFound",
                    "MemberAlreadyExists",
                    "NotExternallyAddressable",
                    "InvalidTransition",
                    "WiringError",
                    "SupervisorRotationIncomplete",
                    "BridgeCommandRejected",
                    "MemberRestoreFailed",
                    "KickoffWaitTimedOut",
                    "ReadyWaitTimedOut",
                    "DefinitionError",
                    "FlowNotFound",
                    "FlowFailed",
                    "RunNotFound",
                    "RunCanceled",
                    "FlowTurnTimedOut",
                    "FrameDepthLimitExceeded",
                    "FrameAtomicPersistenceUnavailable",
                    "SpecRevisionConflict",
                    "SchemaValidation",
                    "InsufficientTargets",
                    "TopologyViolation",
                    "BridgeDeliveryRejected",
                    "SupervisorEscalation",
                    "UnsupportedForMode",
                    "MissingMemberCapability",
                    "ResetBarrier",
                    "StorageError",
                    "SessionError",
                    "CommsError",
                    "CallbackPending",
                    "StaleFenceToken",
                    "StaleEventCursor",
                    "WorkNotFound",
                    "Internal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MobSpawnManyFailureCauseKind",
                &[
                    "ProfileNotFound",
                    "MemberNotFound",
                    "MemberAlreadyExists",
                    "NotExternallyAddressable",
                    "InvalidTransition",
                    "WiringError",
                    "BridgeCommandRejected",
                    "MemberRestoreFailed",
                    "KickoffWaitTimedOut",
                    "ReadyWaitTimedOut",
                    "DefinitionError",
                    "FlowNotFound",
                    "FlowFailed",
                    "RunNotFound",
                    "RunCanceled",
                    "FlowTurnTimedOut",
                    "FrameDepthLimitExceeded",
                    "FrameAtomicPersistenceUnavailable",
                    "SpecRevisionConflict",
                    "SchemaValidation",
                    "InsufficientTargets",
                    "TopologyViolation",
                    "BridgeDeliveryRejected",
                    "SupervisorEscalation",
                    "UnsupportedForMode",
                    "MissingMemberCapability",
                    "ResetBarrier",
                    "StorageError",
                    "SessionError",
                    "CommsError",
                    "CallbackPending",
                    "StaleFenceToken",
                    "StaleEventCursor",
                    "WorkNotFound",
                    "Internal",
                ],
            ),
            NamedTypeBinding::type_path(
                "MobToolCallerProvenance",
                "meerkat_core::service::MobToolCallerProvenance",
            ),
            NamedTypeBinding::string_enum("MobMemberState", &["Active", "Retiring"]),
            NamedTypeBinding::string_enum(
                "MemberWaitClassificationKind",
                &["RuntimeMaterialPresent", "MissingRuntimeMaterial"],
            ),
            NamedTypeBinding::string_enum(
                "MobPhase",
                &["Running", "Stopped", "Completed", "Destroyed"],
            ),
            NamedTypeBinding::string_enum(
                "SubmitWorkRejectReasonKind",
                &[
                    "MobNotRunning",
                    "MemberNotFound",
                    "StaleFenceToken",
                    "NotExternallyAddressable",
                ],
            ),
            NamedTypeBinding::string_enum(
                "NodeRunStatus",
                &[
                    "Pending",
                    "Ready",
                    "Running",
                    "Completed",
                    "Failed",
                    "Skipped",
                    "Canceled",
                ],
            ),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("StepId"),
            NamedTypeBinding::type_path(
                "OpaquePrincipalToken",
                "meerkat_core::service::OpaquePrincipalToken",
            ),
            NamedTypeBinding::string_enum(
                "StepRunStatus",
                &["Dispatched", "Completed", "Failed", "Skipped", "Canceled"],
            ),
            NamedTypeBinding::string_enum(
                "SpawnPolicyRuntimeMode",
                &["AutonomousHost", "TurnDriven"],
            ),
            NamedTypeBinding::type_path(
                "SupervisorProtocolVersion",
                "crate::catalog::dsl::mob_machine::SupervisorProtocolVersion",
            ),
            NamedTypeBinding::string_enum(
                "RespawnTopologyRestoreResultKind",
                &["Completed", "TopologyRestoreFailed"],
            ),
            NamedTypeBinding::string("RespawnTopologyPeerId"),
            NamedTypeBinding::string("WiringEdge"),
            NamedTypeBinding::string_enum("WiringLifecycleKind", &["Wired", "Unwired"]),
            NamedTypeBinding::string("WorkId"),
            NamedTypeBinding::string_enum("WorkOrigin", &["External", "Internal", "Ingest"]),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::mob_machine::PeerAddress",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::mob_machine::PeerId"),
            NamedTypeBinding::type_path("PeerName", "crate::catalog::dsl::mob_machine::PeerName"),
            NamedTypeBinding::type_path(
                "PeerSigningKey",
                "crate::catalog::dsl::mob_machine::PeerSigningKey",
            ),
            // Mob coordination board types (folded from the former standalone
            // MobCoordinationLifecycleAuthorityMachine).
            NamedTypeBinding::string("WorkIntentId"),
            NamedTypeBinding::string("ResourceClaimId"),
            NamedTypeBinding::string("CoordinationResourceRef"),
            NamedTypeBinding::string_enum(
                "MobCoordinationWorkIntentStatus",
                &["Planned", "Active", "Blocked", "Completed", "Cancelled"],
            ),
            NamedTypeBinding::string_enum(
                "MobCoordinationResourceClaimStatus",
                &["Active", "Released", "Expired", "Cancelled"],
            ),
            NamedTypeBinding::string_enum(
                "MobCoordinationResourceClaimKind",
                &["Advisory", "SoftReservation", "Exclusive"],
            ),
            NamedTypeBinding::string_enum(
                "MobCoordinationEventKind",
                &[
                    "WorkIntentRecorded",
                    "WorkIntentStatusChanged",
                    "ResourceClaimRecorded",
                    "ResourceClaimStatusChanged",
                    "ResourceClaimOverlapObserved",
                ],
            ),
        ],
        input_variant_ids(MOB_MACHINE_RUNTIME_INTERNAL_INPUTS),
    )
    .with_ci_step_limit(1)
}

runtime_internal_inputs!(
    MobMachineRuntimeInternalInput,
    MOB_MACHINE_RUNTIME_INTERNAL_INPUTS,
    mob_machine::MobMachineInputVariant,
    [
        AuthorizeFlowFrameReducerCommand,
        AuthorizeFlowRunReducerCommand,
        AuthorizeLoopIterationReducerCommand,
        AuthorizeSpawnProfile,
        ClassifyFlowRunTerminality,
        ClassifyFlowStepTerminality,
        ClassifyFlowFrameTerminalStatus,
        ClassifyFlowRunPublicResult,
        BindOwnerBridgeSession,
        ClassifyMemberWait,
        ClassifySpawnManyFailure,
        ResolveFlowDelegationEdgeAdmission,
        ClassifyRemoteMemberRuntimeObservation,
        ResolveSpawnMemberAdmission,
        ResolveCurrentMobAdmission,
        ResolveCreateMobAdmission,
        ResolveProfileMutationAdmission,
        ClassifyBridgeRejectionRecovery,
        ClassifyPendingSupervisorAcceptance,
        CreateFrameSeed,
        CreateLoopSeed,
        CreateRunSeed,
        KickoffCancelRequested,
        KickoffClear,
        KickoffMarkPending,
        KickoffMarkStarting,
        KickoffResolveCallbackPending,
        KickoffResolveFailed,
        KickoffResolveStarted,
        RecordLoopBodyFrameCompleted,
        RecordLoopUntilConditionFailed,
        RecordLoopUntilConditionMet,
        RequestPendingSessionIngressDetachForMobDestroy,
        RetireAbsent,
        ResolveCancelAllWorkRejection,
        ResolveSubmitWorkRejection,
        SubscribeStructuralEvents,
        RegisterMemberPeer,
        ResolveSpawnPolicy,
        StartupMarkReady,
        SessionIngressDetachFailedForMobDestroy,
        SessionIngressDetachedForMobDestroy,
        AuthorizeMobEventRouterMemberSubscription,
        AuthorizeMobEventRouterMemberRemoval,
        PollEventsStrict,
        AuthorizeMemberPeerOverlay,
        AuthorizeMemberPeerRebind,
        AuthorizeMemberTrustWiring,
        AuthorizeMemberTrustUnwiring,
        AuthorizeMemberTrustCleanup,
        AuthorizeMemberTrustCleanupObserved,
        AuthorizeExternalPeerReciprocalTrust,
        ProvisionSupervisorAuthority,
        ClearSupervisorPendingRotation,
        RecordSupervisorPendingRotation,
        CommitSupervisorRotation,
        ClearSupervisorAuthorityForDestroy,
        RestoreSupervisorAuthorityAfterDestroyRollback,
        RecordCoordinationWorkIntent,
        RecordCoordinationResourceClaim,
        UpdateCoordinationWorkIntentStatus,
        UpdateCoordinationResourceClaimStatus,
        ObserveCoordinationResourceClaimOverlap,
    ]
);

pub fn mob_machine_runtime_internal_input_variants() -> Vec<mob_machine::MobMachineInputVariant> {
    MOB_MACHINE_RUNTIME_INTERNAL_INPUTS
        .iter()
        .copied()
        .map(MobMachineRuntimeInternalInput::input_variant)
        .collect()
}

pub fn dsl_schedule_lifecycle_machine() -> MachineSchema {
    schedule_lifecycle_schema_metadata()
        .attach_to(schedule_lifecycle::ScheduleLifecycleMachineState::schema())
}

pub fn schedule_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        // Schedule-side reciprocal ack tracks `Set<OccurrenceId>` and
        // receives `ConfirmOccurrencesSuperseded { occurrence_id }` from
        // the occurrence authority; both sides must agree on the atom.
        vec![
            NamedTypeBinding::string_enum("MisfirePolicy", &["Skip", "CatchUpWithin"]),
            NamedTypeBinding::string_enum("MissingTargetPolicy", &["MarkMisfired", "Skip"]),
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string_enum("OverlapPolicy", &["AllowConcurrent", "SkipIfRunning"]),
            NamedTypeBinding::string("ScheduleId"),
            NamedTypeBinding::string_enum(
                "ScheduleLifecycleState",
                &["Active", "Paused", "Deleted"],
            ),
        ],
        vec![],
    )
}

pub fn dsl_occurrence_lifecycle_machine() -> MachineSchema {
    occurrence_lifecycle_schema_metadata()
        .attach_to(occurrence_lifecycle::OccurrenceLifecycleMachineState::schema())
}

pub fn occurrence_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string("ClaimToken"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string_enum(
                "DeliveryReceiptStage",
                &[
                    "Planned",
                    "Claimed",
                    "DispatchStarted",
                    "DispatchAccepted",
                    "AwaitingCompletion",
                    "Completed",
                    "Skipped",
                    "Misfired",
                    "Superseded",
                    "DeliveryFailed",
                    "LeaseExpired",
                ],
            ),
            NamedTypeBinding::string_enum(
                "DeliveryCompletionFailureReason",
                &[
                    "CompletionFutureFailed",
                    "RuntimeCompletionChannelClosed",
                    "RuntimeCompletionAuthorityUnavailable",
                    "RuntimeCompletionHandleMissing",
                ],
            ),
            NamedTypeBinding::string_enum(
                "DeliveryFailureReason",
                &[
                    "TargetMaterializationFailed",
                    "TargetMissing",
                    "TargetBusy",
                    "RuntimeRejected",
                    "MobRejected",
                    "TransportError",
                    "InternalError",
                ],
            ),
            NamedTypeBinding::string_enum("MisfirePolicy", &["Skip", "CatchUpWithin"]),
            NamedTypeBinding::string_enum("MissingTargetPolicy", &["MarkMisfired", "Skip"]),
            NamedTypeBinding::string_enum(
                "OccurrenceFailureClass",
                &[
                    "TargetMaterializationFailed",
                    "TargetMissing",
                    "TargetBusy",
                    "RuntimeRejected",
                    "MobRejected",
                    "LeaseLost",
                    "TransportError",
                    "InternalError",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OccurrenceTargetProbeOutcome",
                &["Ready", "Busy", "Missing"],
            ),
            NamedTypeBinding::string_enum(
                "ClaimedDispatchSchedulePhase",
                &["Active", "Paused", "Deleted"],
            ),
            NamedTypeBinding::string_enum(
                "ClaimedDispatchDisposition",
                &["Frozen", "Supersede", "Ready", "FutureRevision"],
            ),
            NamedTypeBinding::string_enum(
                "CompletionSupersessionDisposition",
                &["Supersede", "Proceed"],
            ),
            NamedTypeBinding::string_enum(
                "OccurrenceLifecycleInputVariant",
                &[
                    "PlanOccurrence",
                    "SyncTargetSnapshot",
                    "RecordReceipt",
                    "ClassifyDue",
                    "ClassifyOccurrenceTerminality",
                    "ClassifyClaimedDispatchDisposition",
                    "ClassifyCompletionSupersession",
                    "Claim",
                    "DispatchStarted",
                    "AwaitCompletion",
                    "Complete",
                    "ResolveRuntimeCompletion",
                    "ResolveDeliveryCompletionFailure",
                    "ResolveDeliveryFailure",
                    "ResolveTargetProbe",
                    "ResolveDueMisfire",
                    "Supersede",
                    "LeaseExpired",
                    "ReleaseLeaseForPausedSchedule",
                    "ClassifyTransitionFailure",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OccurrenceTransitionFailureClassKind",
                &[
                    "PlanRejected",
                    "TargetSyncRejected",
                    "ReceiptRecordRejected",
                    "DueClassificationRejected",
                    "ClaimedDispatchClassificationRejected",
                    "CompletionSupersessionClassificationRejected",
                    "ClaimRejected",
                    "NotPendingForClaim",
                    "NotClaimed",
                    "NotDispatching",
                    "NotLeaseHolding",
                    "NotLiveForTerminal",
                ],
            ),
            NamedTypeBinding::string_enum(
                "OccurrenceTransitionFailureRefusalKind",
                &["NoMatchingTransition", "GuardRejected"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCompletionOutcome",
                &[
                    "Completed",
                    "CallbackPending",
                    "Cancelled",
                    "Abandoned",
                    "FinalizationFailed",
                    "RuntimeTerminated",
                ],
            ),
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string_enum("OverlapPolicy", &["AllowConcurrent", "SkipIfRunning"]),
            NamedTypeBinding::string_enum(
                "OccurrenceLifecycleState",
                &[
                    "Pending",
                    "Claimed",
                    "Dispatching",
                    "AwaitingCompletion",
                    "Completed",
                    "Skipped",
                    "Misfired",
                    "Superseded",
                    "DeliveryFailed",
                ],
            ),
            NamedTypeBinding::string("ScheduleId"),
        ],
        vec![],
    )
    .with_ci_step_limit(4)
}

pub fn dsl_workgraph_lifecycle_machine() -> MachineSchema {
    workgraph_lifecycle_schema_metadata()
        .attach_to(workgraph_lifecycle::WorkGraphLifecycleMachineState::schema())
}

pub fn dsl_work_attention_lifecycle_machine() -> MachineSchema {
    work_attention_lifecycle_schema_metadata()
        .attach_to(work_attention_lifecycle::WorkAttentionLifecycleMachineState::schema())
}

pub fn dsl_work_attention_lifecycle_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_work_attention_lifecycle_machine(),
        WORK_ATTENTION_LIFECYCLE_PRODUCTION_RUST_CRATE,
        WORK_ATTENTION_LIFECYCLE_PRODUCTION_RUST_MODULE,
    )
}

pub fn dsl_work_graph_lifecycle_machine() -> MachineSchema {
    dsl_workgraph_lifecycle_machine()
}

pub fn dsl_workgraph_lifecycle_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_workgraph_lifecycle_machine(),
        WORKGRAPH_LIFECYCLE_PRODUCTION_RUST_CRATE,
        WORKGRAPH_LIFECYCLE_PRODUCTION_RUST_MODULE,
    )
}

pub fn work_attention_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string("WorkAttentionBindingKey"),
            NamedTypeBinding::string_enum(
                "WorkAttentionLifecycleState",
                &["Active", "Paused", "Superseded", "Stopped"],
            ),
            NamedTypeBinding::string_enum(
                "WorkAttentionMode",
                &[
                    "Pursue",
                    "Coordinate",
                    "Review",
                    "Falsify",
                    "Judge",
                    "Observe",
                ],
            ),
            NamedTypeBinding::string_enum(
                "AttentionDelegatedAuthority",
                &[
                    "AddEvidence",
                    "CloseOwnReviewItem",
                    "RequestClosure",
                    "CloseIfPolicyAllows",
                ],
            ),
        ],
        vec![],
    )
}

pub fn workgraph_lifecycle_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string_enum(
                "WorkLifecycleState",
                &[
                    "Absent",
                    "Open",
                    "InProgress",
                    "Blocked",
                    "Completed",
                    "Cancelled",
                    "Failed",
                ],
            ),
            NamedTypeBinding::string("WorkItemKey"),
            NamedTypeBinding::type_path_struct(
                "WorkEdgeKey",
                "crate::catalog::dsl::workgraph_lifecycle::WorkEdgeKey",
                vec![
                    TypePathStructField::named("kind", "WorkEdgeKind"),
                    TypePathStructField::named("from_item_key", "WorkItemKey"),
                    TypePathStructField::named("to_item_key", "WorkItemKey"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "WorkDependencyPathKey",
                "crate::catalog::dsl::workgraph_lifecycle::WorkDependencyPathKey",
                vec![
                    TypePathStructField::named("kind", "WorkEdgeKind"),
                    TypePathStructField::named("from_item_key", "WorkItemKey"),
                    TypePathStructField::named("to_item_key", "WorkItemKey"),
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkEdgeKind",
                &["Blocks", "Parent", "Related", "Supersedes", "DerivedFrom"],
            ),
            NamedTypeBinding::string_enum(
                "WorkCompletionPolicy",
                &[
                    "SelfAttest",
                    "HostConfirmed",
                    "PrincipalConfirmed",
                    "Supervisor",
                    "ReviewerQuorum",
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkOwnerKind",
                &["Principal", "Agent", "Session", "Mob", "Label"],
            ),
            NamedTypeBinding::string_enum(
                "WorkEvidenceKind",
                &[
                    "SelfAttest",
                    "HostConfirmation",
                    "PrincipalConfirmation",
                    "SupervisorConfirmation",
                    "ReviewerConfirmation",
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkGraphErrorKind",
                &[
                    "NotFound",
                    "AttentionNotFound",
                    "StaleRevision",
                    "Conflict",
                    "InvalidTransition",
                    "InvalidInput",
                    "InvalidTimestampMillis",
                    "Store",
                    "UnsupportedBackend",
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkGraphPublicErrorClass",
                &[
                    "NotFound",
                    "Conflict",
                    "InvalidTransition",
                    "InvalidArguments",
                    "CapabilityUnavailable",
                    "StoreError",
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkCreateStatusAdmissionKind",
                &["Denied", "AdmittedOpen", "AdmittedBlocked"],
            ),
            NamedTypeBinding::string_enum(
                "WorkPublicConfirmationAdmissionKind",
                &["DeniedRequiresTrustedHost", "Admitted"],
            ),
            NamedTypeBinding::string_enum(
                "WorkCompletionPolicyMutationAdmissionKind",
                &["Denied", "Admitted"],
            ),
            NamedTypeBinding::type_path_struct(
                "WorkOwnerKey",
                "crate::catalog::dsl::workgraph_lifecycle::WorkOwnerKey",
                vec![
                    TypePathStructField::named("kind", "WorkOwnerKind"),
                    TypePathStructField::string("id"),
                ],
            ),
        ],
        vec![],
    )
}
