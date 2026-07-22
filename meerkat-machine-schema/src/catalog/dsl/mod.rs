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
pub mod mob_host_binding_authority;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;
pub mod session_document;
pub mod session_persistence_version_authority;
pub mod session_turn_admission;
pub mod work_attention_lifecycle;
pub mod workgraph_lifecycle;

use crate::identity::{EffectVariantId, InputVariantId, SignalVariantId, TransitionId};
use crate::{
    CommandPlanSchema, EffectClosureSchema, MachineSchema, NamedTypeBinding, RustBinding,
    TypePathEnumPayloadField, TypePathEnumStructuralVariant, TypePathStructField,
};

pub struct MachineSchemaMetadata {
    pub named_types: Vec<NamedTypeBinding>,
    pub runtime_internal_inputs: Vec<InputVariantId>,
    pub tlc_representative_inputs: Vec<InputVariantId>,
    pub command_plans: Vec<CommandPlanSchema>,
    pub ci_step_limit: Option<u32>,
    pub deep_domain_overrides: std::collections::BTreeMap<String, usize>,
}

impl MachineSchemaMetadata {
    pub fn attach_to(self, mut schema: MachineSchema) -> MachineSchema {
        schema.named_types = self.named_types;
        schema.runtime_internal_inputs = self.runtime_internal_inputs;
        schema.tlc_representative_inputs = self.tlc_representative_inputs;
        schema.command_plans = self.command_plans;
        schema.ci_step_limit = self.ci_step_limit;
        schema.deep_domain_overrides = self.deep_domain_overrides;
        schema
    }

    pub fn with_ci_step_limit(mut self, ci_step_limit: u32) -> Self {
        self.ci_step_limit = Some(ci_step_limit);
        self
    }

    /// Mark one state-independent input for singleton payload sampling in TLC
    /// lifecycle exploration. Runtime codegen and the input alphabet are not
    /// changed by this model-checking-only annotation.
    pub fn with_tlc_representative_input(mut self, input: InputVariantId) -> Self {
        self.tlc_representative_inputs.push(input);
        self
    }

    /// Deep-profile TLC sample-cardinality override for one generated
    /// CONSTANT domain (model-checker configuration, not vocabulary).
    pub fn with_deep_domain_override(
        mut self,
        domain: impl Into<String>,
        cardinality: usize,
    ) -> Self {
        self.deep_domain_overrides
            .insert(domain.into(), cardinality);
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
pub const MOB_HOST_BINDING_AUTHORITY_PRODUCTION_RUST_CRATE: &str = "meerkat-mob";
pub const MOB_HOST_BINDING_AUTHORITY_PRODUCTION_RUST_MODULE: &str =
    "machines::mob_host_binding_authority";

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

fn input_variant_id(value: &'static str) -> InputVariantId {
    InputVariantId::from_trusted_catalog_literal(value)
}

fn signal_variant_id(value: &'static str) -> SignalVariantId {
    SignalVariantId::from_trusted_catalog_literal(value)
}

fn transition_id(value: &'static str) -> TransitionId {
    TransitionId::from_trusted_catalog_literal(value)
}

fn transition_id_owned(value: String) -> TransitionId {
    TransitionId::from_trusted_catalog_string(value)
}

fn effect_variant_id(value: &'static str) -> EffectVariantId {
    EffectVariantId::from_trusted_catalog_literal(value)
}

fn machine_schema_metadata(
    named_types: Vec<NamedTypeBinding>,
    runtime_internal_inputs: Vec<InputVariantId>,
) -> MachineSchemaMetadata {
    MachineSchemaMetadata {
        named_types,
        runtime_internal_inputs,
        tlc_representative_inputs: Vec::new(),
        command_plans: Vec::new(),
        ci_step_limit: None,
        deep_domain_overrides: std::collections::BTreeMap::new(),
    }
}

fn meerkat_queue_to_run_command_plans() -> Vec<CommandPlanSchema> {
    let admission_plan_transitions = [
        "ResolveAdmissionPlanRequestedTerminalQueueIdle",
        "ResolveAdmissionPlanRequestedTerminalQueueAttached",
        "ResolveAdmissionPlanRequestedTerminalQueueRunning",
        "ResolveAdmissionPlanRequestedTerminalSteerIdle",
        "ResolveAdmissionPlanRequestedTerminalSteerAttached",
        "ResolveAdmissionPlanRequestedTerminalSteerRunning",
        "ResolveAdmissionPlanRequestedQueueIdle",
        "ResolveAdmissionPlanRequestedQueueAttached",
        "ResolveAdmissionPlanRequestedQueueRunning",
        "ResolveAdmissionPlanRequestedSteerIdle",
        "ResolveAdmissionPlanRequestedSteerAttached",
        "ResolveAdmissionPlanRequestedSteerRunning",
        "ResolveAdmissionPlanDefaultQueueKindIdle",
        "ResolveAdmissionPlanDefaultQueueKindAttached",
        "ResolveAdmissionPlanDefaultQueueKindRunning",
        "ResolveAdmissionPlanDefaultPeerMessageOrRequestIdle",
        "ResolveAdmissionPlanDefaultPeerMessageOrRequestAttached",
        "ResolveAdmissionPlanDefaultPeerMessageOrRequestRunning",
        "ResolveAdmissionPlanPeerResponseProgressIdle",
        "ResolveAdmissionPlanPeerResponseProgressAttached",
        "ResolveAdmissionPlanPeerResponseProgressRunning",
        "ResolveAdmissionPlanDefaultPeerResponseTerminalIdle",
        "ResolveAdmissionPlanDefaultPeerResponseTerminalAttached",
        "ResolveAdmissionPlanDefaultPeerResponseTerminalRunning",
        "ResolveAdmissionPlanDefaultContinuationIdle",
        "ResolveAdmissionPlanDefaultContinuationAttached",
        "ResolveAdmissionPlanDefaultContinuationRunning",
        "ResolveAdmissionPlanWorkgraphAttentionContinuationIdle",
        "ResolveAdmissionPlanWorkgraphAttentionContinuationAttached",
        "ResolveAdmissionPlanWorkgraphAttentionContinuationRunning",
        "ResolveAdmissionPlanOperationIdle",
        "ResolveAdmissionPlanOperationAttached",
        "ResolveAdmissionPlanOperationRunning",
        "QueueAcceptedIdle",
        "QueueAcceptedAttached",
        "QueueAcceptedRunning",
        "QueueAcceptedRetired",
        "QueueAcceptedStopped",
        "SteerAcceptedIdle",
        "SteerAcceptedAttached",
        "SteerAcceptedRunning",
        "SteerAcceptedRetired",
        "SteerAcceptedStopped",
    ];
    let stage_transitions = [
        "StageForRunIdle",
        "StageForRunAttached",
        "StageForRunRunning",
        "StageForRunRetired",
        "StageForRunStopped",
    ];
    let run_commit_transitions = [
        "RunCompleted",
        "RunFailed",
        "RunCancelled",
        "CommitRunningToIdle",
        "CommitRunningToAttached",
        "CommitRunningToRetired",
        "FailRunningToIdle",
        "FailRunningToAttached",
        "FailRunningToRetired",
        "CancelRunningToIdle",
        "CancelRunningToAttached",
        "CancelRunningToRetired",
        "RollbackRunRunningToIdle",
        "RollbackRunRunningToAttached",
        "RollbackRunRunningToRetired",
    ];
    let completion_result_phases = [
        "Initializing",
        "Idle",
        "Attached",
        "Running",
        "Retired",
        "Stopped",
    ];
    let completion_result_families = [
        "ResolveRuntimeCompletionResultCompleted",
        "ResolveRuntimeCompletionResultWithoutResult",
        "ResolveRuntimeCompletionResultCallbackPending",
        "ResolveRuntimeCompletionResultCancelled",
        "ResolveRuntimeCompletionResultRuntimeApplyFailed",
        "ResolveRuntimeCompletionResultMachineFailed",
        "ResolveRuntimeCompletionResultFinalizationFailureWithResult",
        "ResolveRuntimeCompletionResultFinalizationFailureWithoutResult",
        "ResolveRuntimeCompletionResultRuntimeTerminated",
    ];
    let mut completion_result_transitions = completion_result_families
        .iter()
        .flat_map(|family| {
            completion_result_phases
                .iter()
                .map(move |phase| transition_id_owned(format!("{family}{phase}")))
        })
        .collect::<Vec<_>>();
    completion_result_transitions.push(transition_id(
        "ResolveRuntimeCompletionResultRuntimeTerminatedDestroyedDestroyed",
    ));
    vec![
        CommandPlanSchema {
            name: "AuthorizedAcceptedInputMaterialization".to_owned(),
            authority_type: "AuthorizedAcceptedInputMaterialization".to_owned(),
            source_inputs: vec![
                input_variant_id("ResolveAdmissionPlan"),
                input_variant_id("QueueAccepted"),
                input_variant_id("SteerAccepted"),
            ],
            source_signals: vec![],
            transitions: admission_plan_transitions
                .iter()
                .copied()
                .map(transition_id)
                .collect(),
            effects: vec![
                effect_variant_id("AdmissionResolved"),
                effect_variant_id("IngressAccepted"),
                effect_variant_id("PostAdmissionSignal"),
            ],
            effect_closures: vec![],
        },
        CommandPlanSchema {
            name: "AuthorizeRuntimeLoopBatch".to_owned(),
            authority_type: "AuthorizedRuntimeLoopBatch".to_owned(),
            source_inputs: vec![
                input_variant_id("QueueAccepted"),
                input_variant_id("SteerAccepted"),
                input_variant_id("RecoverAdmittedInput"),
                input_variant_id("RecoverInputLifecycle"),
            ],
            source_signals: vec![],
            transitions: vec![
                transition_id("QueueAcceptedIdle"),
                transition_id("QueueAcceptedAttached"),
                transition_id("QueueAcceptedRunning"),
                transition_id("QueueAcceptedRetired"),
                transition_id("QueueAcceptedStopped"),
                transition_id("SteerAcceptedIdle"),
                transition_id("SteerAcceptedAttached"),
                transition_id("SteerAcceptedRunning"),
                transition_id("SteerAcceptedRetired"),
                transition_id("SteerAcceptedStopped"),
                transition_id("RecoverInputLifecycleIdle"),
                transition_id("RecoverInputLifecycleAttached"),
                transition_id("RecoverInputLifecycleRunning"),
                transition_id("RecoverInputLifecycleRetired"),
                transition_id("RecoverInputLifecycleStopped"),
            ],
            effects: vec![effect_variant_id("IngressAccepted")],
            effect_closures: vec![],
        },
        CommandPlanSchema {
            name: "AuthorizedStageForRun".to_owned(),
            authority_type: "AuthorizedStageForRun".to_owned(),
            source_inputs: vec![input_variant_id("StageForRun")],
            source_signals: vec![],
            transitions: stage_transitions
                .iter()
                .copied()
                .map(transition_id)
                .collect(),
            effects: vec![],
            effect_closures: vec![],
        },
        CommandPlanSchema {
            name: "AuthorizedRuntimeLoopRunCommit".to_owned(),
            authority_type: "AuthorizedRuntimeLoopRunCommit".to_owned(),
            source_inputs: vec![
                input_variant_id("RunCompleted"),
                input_variant_id("RunFailed"),
                input_variant_id("RunCancelled"),
                input_variant_id("Commit"),
                input_variant_id("Fail"),
                input_variant_id("CancelRun"),
                input_variant_id("RollbackRun"),
            ],
            source_signals: vec![],
            transitions: run_commit_transitions
                .iter()
                .copied()
                .map(transition_id)
                .collect(),
            effects: vec![
                effect_variant_id("TurnRunCompleted"),
                effect_variant_id("TurnRunFailed"),
                effect_variant_id("TurnRunCancelled"),
            ],
            effect_closures: vec![
                EffectClosureSchema {
                    effect: effect_variant_id("TurnRunCompleted"),
                    authority_type: "AuthorizedRuntimeLoopRunCommit".to_owned(),
                    closure_policy: "RuntimeLoopRunCommitEffect".to_owned(),
                    lifecycle: vec![
                        "Authorized".to_owned(),
                        "Attempted".to_owned(),
                        "Realized".to_owned(),
                        "Failed".to_owned(),
                        "Cancelled".to_owned(),
                        "Abandoned".to_owned(),
                    ],
                },
                EffectClosureSchema {
                    effect: effect_variant_id("TurnRunFailed"),
                    authority_type: "AuthorizedRuntimeLoopRunCommit".to_owned(),
                    closure_policy: "RuntimeLoopRunCommitEffect".to_owned(),
                    lifecycle: vec![
                        "Authorized".to_owned(),
                        "Attempted".to_owned(),
                        "Realized".to_owned(),
                        "Failed".to_owned(),
                        "Cancelled".to_owned(),
                        "Abandoned".to_owned(),
                    ],
                },
                EffectClosureSchema {
                    effect: effect_variant_id("TurnRunCancelled"),
                    authority_type: "AuthorizedRuntimeLoopRunCommit".to_owned(),
                    closure_policy: "RuntimeLoopRunCommitEffect".to_owned(),
                    lifecycle: vec![
                        "Authorized".to_owned(),
                        "Attempted".to_owned(),
                        "Realized".to_owned(),
                        "Failed".to_owned(),
                        "Cancelled".to_owned(),
                        "Abandoned".to_owned(),
                    ],
                },
            ],
        },
        CommandPlanSchema {
            name: "AuthorizedInteractionTerminalOutboxAdoption".to_owned(),
            authority_type: "AuthorizedInteractionTerminalOutboxAdoption".to_owned(),
            source_inputs: vec![input_variant_id(
                "AuthorizeInteractionTerminalOutboxAdoption",
            )],
            source_signals: vec![],
            transitions: completion_result_phases
                .iter()
                .map(|phase| {
                    transition_id_owned(format!(
                        "AuthorizeInteractionTerminalOutboxAdoption{phase}"
                    ))
                })
                .collect(),
            effects: vec![effect_variant_id(
                "InteractionTerminalOutboxAdoptionAuthorized",
            )],
            effect_closures: vec![EffectClosureSchema {
                effect: effect_variant_id("InteractionTerminalOutboxAdoptionAuthorized"),
                authority_type: "AuthorizedInteractionTerminalOutboxAdoption".to_owned(),
                closure_policy: "DurableOutboxBindingAdoption".to_owned(),
                lifecycle: vec![
                    "Authorized".to_owned(),
                    "Attempted".to_owned(),
                    "Realized".to_owned(),
                    "Failed".to_owned(),
                    "Abandoned".to_owned(),
                ],
            }],
        },
        CommandPlanSchema {
            name: "AuthorizedRuntimeCompletionResultClosure".to_owned(),
            authority_type: "RuntimeCompletionResultAuthority".to_owned(),
            source_inputs: vec![input_variant_id("ResolveRuntimeCompletionResult")],
            source_signals: vec![],
            transitions: completion_result_transitions,
            effects: vec![effect_variant_id("RuntimeCompletionResultResolved")],
            effect_closures: vec![EffectClosureSchema {
                effect: effect_variant_id("RuntimeCompletionResultResolved"),
                authority_type: "RuntimeCompletionResultAuthority".to_owned(),
                closure_policy: "LocalSurfaceResultAlignment".to_owned(),
                lifecycle: vec![
                    "Authorized".to_owned(),
                    "Attempted".to_owned(),
                    "Realized".to_owned(),
                    "Failed".to_owned(),
                    "Cancelled".to_owned(),
                    "Abandoned".to_owned(),
                ],
            }],
        },
    ]
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
                    "RefreshDisallowed",
                    "ReauthRequired",
                    "LeaseAbsent",
                    "AlreadyRefreshing",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RefreshFailureDisposition",
                &["Transient", "ReauthRequired"],
            ),
        ],
        vec![
            InputVariantId::from_trusted_catalog_literal("RestoreAuthoritySnapshot"),
            InputVariantId::from_trusted_catalog_literal("RestoreCredentialLifecycleSnapshot"),
            InputVariantId::from_trusted_catalog_literal("RestoreOAuthBrowserFlow"),
            InputVariantId::from_trusted_catalog_literal("RestoreOAuthDeviceFlow"),
            InputVariantId::from_trusted_catalog_literal("RestoreOAuthDevicePoll"),
            InputVariantId::from_trusted_catalog_literal("ResolveRefreshFailureDisposition"),
            InputVariantId::from_trusted_catalog_literal("ResolveCredentialUseAdmission"),
            InputVariantId::from_trusted_catalog_literal("ResolveOAuthLoginCredentialDisposition"),
        ],
    )
    .with_ci_step_limit(3)
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
            // WAVE G1 fold (#86): machine-owned transcript edit directive.
            NamedTypeBinding::string_enum("TranscriptEditKind", &["Fork", "Rewrite"]),
            NamedTypeBinding::string_enum(
                "SessionFirstTurnPhase",
                &["Inactive", "Pending", "Consumed"],
            ),
            NamedTypeBinding::string_enum("SessionInitialPromptStageDecision", &["Clear", "Store"]),
            NamedTypeBinding::string_enum(
                "SystemContextAppendDecision",
                &["Staged", "Duplicate", "RejectEmpty", "RejectConflict"],
            ),
            NamedTypeBinding::string_enum(
                "SystemContextPersistAppendAdmission",
                &["Reject", "Admit"],
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
            NamedTypeBinding::string_enum(
                "RealtimeUserContentIdentityDisposition",
                &[
                    "RejectInvalidIdentity",
                    "RejectUnmaterializedPredecessor",
                    "RejectConflict",
                    "AlreadyCommitted",
                    "CommitNew",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeUserContentBlobStageDisposition",
                &["RejectOccupied", "StageNew", "ReuseExact"],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeUserContentBlobRecoveryDisposition",
                &[
                    "NoPending",
                    "RetryExact",
                    "CommitVerifiedBeforeCurrent",
                    "ClearInvalidBeforeCurrent",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RealtimeUserContentBlobFinalizeDisposition",
                &["RejectMismatch", "NoPending", "ClearCommitted"],
            ),
            // Durable-config region typed vocabulary (folded from the retired
            // SessionDurableConfigAuthorityMachine).
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
                &["ProviderRequiresModel", "BuildOnlyAfterFirstTurn"],
            ),
            NamedTypeBinding::string_enum(
                "ResumeProviderSelection",
                &["RecomputeFromModel", "UseOverride", "UseStored"],
            ),
            NamedTypeBinding::string_enum(
                "ResumeSelfHostedSelection",
                &["Clear", "UseOverride", "Retain"],
            ),
            NamedTypeBinding::string_enum(
                "LiveSessionAuthorityKind",
                &["LiveAuthoritative", "DurableAuthoritative"],
            ),
            // Runtime-projection-rollback region typed vocabulary (disposition
            // for a runtime-authoritative projection save whose durable row
            // ran ahead of the authority transcript).
            NamedTypeBinding::string_enum(
                "RuntimeProjectionRollbackDisposition",
                &["RejectDivergent", "RebuildToAuthority"],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeCheckpointProjectionDisposition",
                &["IgnoreArchived", "Project"],
            ),
            NamedTypeBinding::string_enum(
                "LiveSessionAuthorityReason",
                &[
                    "StoredArchived",
                    "LiveUncommittedTranscript",
                    "RuntimeSystemContextDiverged",
                    "StoredTranscriptRevisionDiverged",
                ],
            ),
            // Lifecycle-terminal region typed vocabulary (LUC-524 R004 fold:
            // SessionDocumentMachine owns archive lifecycle truth for all
            // profiles).
            NamedTypeBinding::string_enum("SessionDocumentLifecycle", &["Active", "Archived"]),
            NamedTypeBinding::string_enum(
                "SessionArchiveDisposition",
                &["Archive", "AlreadyArchived"],
            ),
            NamedTypeBinding::string_enum(
                "SessionArchiveRuntimeObservation",
                &["Absent", "RetirementRequired", "QuiescentTerminal"],
            ),
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
            // WAVE G1 fold (#345 keep-alive tri-state).
            NamedTypeBinding::string_enum(
                "RuntimeKeepAliveRequest",
                &["Enable", "Disable", "Preserve"],
            ),
            // Dogma K13: machine-resolved persistence decision replaces the
            // former `persist_keep_alive: bool` effect payload that collapsed
            // Disable and Preserve.
            NamedTypeBinding::string_enum(
                "RuntimeKeepAlivePersistenceDecision",
                &["PersistEnabled", "PersistDisabled", "PreserveExisting"],
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
            // 0.7.2 disciplined shell inputs (D2a): machine-owned legality
            // verdict for runtime system-context application, and the typed
            // "session archived" terminal for admission inputs that
            // legitimately race archive/discard teardown.
            NamedTypeBinding::string_enum(
                "RuntimeSystemContextApplicationAuthorization",
                &["Authorized", "SessionArchived"],
            ),
            NamedTypeBinding::string_enum("TurnAdmissionShutdownTerminal", &["SessionArchived"]),
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

runtime_internal_inputs!(
    MobHostBindingAuthorityRuntimeInternalInput,
    MOB_HOST_BINDING_AUTHORITY_RUNTIME_INTERNAL_INPUTS,
    mob_host_binding_authority::MobHostBindingAuthorityInputVariant,
    [
        // Host bind/rebind/revoke + generic host-addressed admission: the host
        // comms acceptor observes envelope facts and feeds them typed; the
        // authority adjudicates (plan §6.3, A11).
        ResolveHostBind,
        ResolveHostRebind,
        RevokeHostBinding,
        ResolveHostCommandAdmission,
        // Materialize dedup/preflight adjudication + success recorder driven
        // by the materialize executor (§14 R7/R8).
        ResolveMaterializeAdmission,
        ResolveMaterializePreflight,
        RecordMaterializedMember,
        // Release admission mirror + disposal recorder (§19.L3).
        ResolveReleaseAdmission,
        RecordMemberRelease,
        // Turn-outcome journal recorder (§18 O2).
        RecordTurnOutcome,
        CancelTrackedInput,
        CompleteTrackedInputCancel,
    ]
);

/// Non-canonical scoped-authority schema (plan §21.5): the member host's
/// mob-keyed binding/admission/dedup authority. Registered for production-
/// schema parity via `meerkat-mob/tests/mob_host_binding_authority.rs`
/// (`production_schema_matches_catalog_schema`), not for canonical gates.
pub fn dsl_mob_host_binding_authority_machine() -> MachineSchema {
    mob_host_binding_authority_schema_metadata()
        .attach_to(mob_host_binding_authority::MobHostBindingAuthorityState::schema())
}

pub fn dsl_mob_host_binding_authority_machine_production_schema() -> MachineSchema {
    with_production_rust_binding(
        dsl_mob_host_binding_authority_machine(),
        MOB_HOST_BINDING_AUTHORITY_PRODUCTION_RUST_CRATE,
        MOB_HOST_BINDING_AUTHORITY_PRODUCTION_RUST_MODULE,
    )
}

pub fn mob_host_binding_authority_schema_metadata() -> MachineSchemaMetadata {
    machine_schema_metadata(
        vec![
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("AgentIdentity"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::type_path(
                "PeerId",
                "crate::catalog::dsl::mob_host_binding_authority::PeerId",
            ),
            NamedTypeBinding::type_path(
                "PeerSigningKey",
                "crate::catalog::dsl::mob_host_binding_authority::PeerSigningKey",
            ),
            NamedTypeBinding::type_path_struct(
                "MemberKey",
                "crate::catalog::dsl::mob_host_binding_authority::MemberKey",
                vec![
                    TypePathStructField::named("mob_id", "MobId"),
                    TypePathStructField::named("agent_identity", "AgentIdentity"),
                ],
            ),
            // DEC-5: the turn journal key is mob-keyed.
            NamedTypeBinding::type_path_struct(
                "TurnKey",
                "crate::catalog::dsl::mob_host_binding_authority::TurnKey",
                vec![
                    TypePathStructField::named("mob_id", "MobId"),
                    TypePathStructField::named("agent_identity", "AgentIdentity"),
                    TypePathStructField::named("generation", "Generation"),
                    TypePathStructField::named("fence_token", "FenceToken"),
                    TypePathStructField::named("input_id", "InputId"),
                ],
            ),
            NamedTypeBinding::string_enum("HostBindingPhase", &["Bound"]),
            NamedTypeBinding::string_enum(
                "HostAdmissionRejectKind",
                &[
                    "NotBound",
                    "StaleSupervisor",
                    "SenderMismatch",
                    "InvalidBootstrapToken",
                    "AddressMismatch",
                    "StaleFence",
                    "AlreadyBound",
                    "Unsupported",
                    "TurnDirectiveUnsupported",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MaterializeRejectKind",
                &[
                    "NotBound",
                    "StaleFence",
                    "SpecDigestMismatch",
                    "ModelUnresolvable",
                    "AuthBindingUnresolvable",
                    "EnvKeysMissing",
                    "McpCommandMissing",
                    "RealmBackendUnavailable",
                    "MemoryStoreUnavailable",
                    "EngineProtocolUnsupported",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MemberSessionDisposal",
                &[
                    "Archived",
                    "RuntimeReleasedOnlyHostOwned",
                    "RuntimeReleasedOnlyNoDurableSessions",
                ],
            ),
            NamedTypeBinding::string_enum(
                "FlowTurnOutcomeKind",
                &["Completed", "Failed", "Canceled"],
            ),
            NamedTypeBinding::string_enum(
                "TrackedInputCancelKind",
                &["NoEffect", "Cancelling", "Cancelled"],
            ),
        ],
        input_variant_ids(MOB_HOST_BINDING_AUTHORITY_RUNTIME_INTERNAL_INPUTS),
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
    let mut metadata = machine_schema_metadata(
        vec![
            NamedTypeBinding::u64("BoundarySequence"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("RuntimeEpochId"),
            NamedTypeBinding::string("CommsRuntimeId"),
            // WAVE G1 fold (#51): machine-owned staged realtime transcript item
            // role/lane classifiers carried on the RealtimeTranscriptAppended effect.
            NamedTypeBinding::string_enum("RealtimeTranscriptRoleKind", &["User", "Assistant"]),
            NamedTypeBinding::string_enum("RealtimeTranscriptLaneKind", &["Display", "Spoken"]),
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
                    "DurabilityMissing",
                    "ExternalDerivedDurabilityForbidden",
                    "DerivedDurabilityForbiddenForInputKind",
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
            NamedTypeBinding::type_path("OperationResult", "meerkat_core::ops::OperationResult"),
            // K8b fold: the ops terminal payload is the domain type itself —
            // `meerkat_core::ops_lifecycle::OperationTerminalOutcome` — carried
            // typed through the machine (no JSON-string codec). The structural
            // variant declarations give the model/oracle a faithful finite
            // domain for variant-matches-kind guards.
            NamedTypeBinding::type_path_enum_with_structural_variants(
                "OpTerminalPayload",
                "meerkat_core::ops_lifecycle::OperationTerminalOutcome",
                &["Retired"],
                vec![
                    TypePathEnumStructuralVariant::with_fields(
                        "Completed",
                        vec![TypePathEnumPayloadField::named("result", "OperationResult")],
                    ),
                    TypePathEnumStructuralVariant::with_fields(
                        "Failed",
                        vec![TypePathEnumPayloadField::string("error")],
                    ),
                    TypePathEnumStructuralVariant::with_fields(
                        "Aborted",
                        vec![TypePathEnumPayloadField::optional_string("reason")],
                    ),
                    TypePathEnumStructuralVariant::with_fields(
                        "Cancelled",
                        vec![TypePathEnumPayloadField::optional_string("reason")],
                    ),
                    TypePathEnumStructuralVariant::with_fields(
                        "Terminated",
                        vec![TypePathEnumPayloadField::string("reason")],
                    ),
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
                    "Abandoned",
                ],
            ),
            NamedTypeBinding::string_enum(
                "InteractionStreamAbandonReason",
                &[
                    "SendFailed",
                    "AdmissionRejected",
                    "ResponseRejected",
                    "TerminalDeliveryFailed",
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
            NamedTypeBinding::string_enum("CallTimeoutSource", &["CallBudget", "TurnBudget"]),
            NamedTypeBinding::string_enum(
                "CallTimeoutVerdict",
                &["RetryableCallTimeout", "TerminalTurnBudget"],
            ),
            NamedTypeBinding::string_enum(
                "LiveOpenAdmissionRejection",
                &["AlreadyBound", "ChannelAlreadyBound", "LifecycleClosed"],
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
                "PeerIngressRequestClass",
                &[
                    "Other",
                    "MobPeerAdded",
                    "MobPeerRetired",
                    "MobPeerUnwired",
                    "SupervisorBridge",
                ],
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
            NamedTypeBinding::string_enum("RegistrationPhase", &["Queuing", "Active", "Draining"]),
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
                &["Accept", "ResumePendingRevoke", "Reject"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBridgeCommandRejectionKind",
                &[
                    "NotBound",
                    "StaleSupervisor",
                    "SenderMismatch",
                    "CommandNotAllowed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorCleanupCommandKind",
                &["Retire", "Observe", "Destroy", "Revoke"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBindAdmissionResultKind",
                &["Bootstrap", "IdempotentAck", "Reject"],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorBindRejectionKind",
                &["AlreadyBound", "SenderMismatch", "RevocationPending"],
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
                "SupervisorRotationPhase",
                &[
                    "PreviousRevokePending",
                    "NextPublishPending",
                    "Completed",
                    "Rejected",
                ],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorRotationSubmissionResultKind",
                &[
                    "New",
                    "ExistingPending",
                    "ExistingTerminal",
                    "Rejected",
                    "Conflict",
                ],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorRotationObservationStatusKind",
                &[
                    "NotFound",
                    "PreviousRevokePending",
                    "NextPublishPending",
                    "Completed",
                    "Rejected",
                ],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorRotationRejectionKind",
                &[
                    "OperationConflict",
                    "NotBound",
                    "SenderMismatch",
                    "TargetEpochNotAdvanced",
                    "InvalidTarget",
                    "UnsupportedProtocolVersion",
                ],
            ),
            NamedTypeBinding::string_enum(
                "SupervisorAuthorizeRejectionKind",
                &[
                    "NotBound",
                    "StaleSupervisor",
                    "SenderMismatch",
                    "RotationNotAllowed",
                ],
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
                    "SkillResolutionFailed",
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
                "RuntimeEventKind",
                &[
                    "InputLifecycle",
                    "RunLifecycle",
                    "RuntimeStateChange",
                    "Topology",
                    "Projection",
                ],
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
                &[
                    "Interrupted",
                    "StagedNoop",
                    "NotFound",
                    "SessionBusy",
                    "Conflict",
                ],
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
                "RuntimeAuthorityObservationKind",
                &[
                    "Missing",
                    "Decoded",
                    "Unsupported",
                    "Malformed",
                    "Unavailable",
                ],
            ),
            NamedTypeBinding::string_enum(
                "RuntimeAuthorityReconcileDecision",
                &[
                    "RepairBlocked",
                    "Converged",
                    "NormalizeOrReplace",
                    "Quarantine",
                    "Backoff",
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
                    TypePathEnumStructuralVariant::named_set("Allow", "names", "ToolName"),
                    TypePathEnumStructuralVariant::named_set("Deny", "names", "ToolName"),
                ],
            ),
            // K8a fold: the tool-visibility name domain is the canonical
            // `meerkat_core::types::ToolName` newtype, threaded through the
            // machine as a named value domain (overlay sets, deferred name
            // sets, witness/catalog map keys, and ToolFilter payloads).
            NamedTypeBinding::type_path("ToolName", "meerkat_core::types::ToolName"),
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
                &["last_seen_provenance"],
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
    .with_ci_step_limit(1);
    metadata.command_plans = meerkat_queue_to_run_command_plans();
    metadata
}

runtime_internal_inputs!(
    MeerkatMachineRuntimeInternalInput,
    MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS,
    meerkat_machine::MeerkatMachineInputVariant,
    [
        AbandonInput,
        AbandonLiveOpenAdmission,
        AbortCancelAfterBoundaryDispatch,
        AbortOp,
        AcknowledgeTerminal,
        AddDirectPeerEndpoint,
        AdvanceSessionContext,
        ApplyMobPeerOverlay,
        AttachMobIngress,
        AttachSessionIngress,
        AuthorizeSupervisor,
        PrepareTerminalSupervisorCleanupBindings,
        RecoverSupervisorBinding,
        RecoverSupervisorRevocationPending,
        RecoverSupervisorRotationOperation,
        RecoverSupervisorRotationTerminalReceipt,
        RecoverRevokedSupervisorReceipt,
        RefreshSupervisorBindingRoute,
        SubmitSupervisorRotation,
        ResumeSupervisorRotation,
        SupervisorRotationPreviousRevoked,
        SupervisorRotationNextPublished,
        ObserveSupervisorRotation,
        ResolveSupervisorCleanupCommandAdmission,
        AuthorizeDeferredSessionSystemContextAppend,
        BeginUnregisterSession,
        BeginUnregisterUnservedAttachment,
        BindSupervisor,
        BoundaryComplete,
        BoundaryContinue,
        BudgetExhausted,
        CallbackPending,
        CancelNow,
        CancelOp,
        CancelRun,
        CancelWaitAll,
        CancellationObserved,
        ChangeLane,
        ClearLocalEndpoint,
        CoalesceInput,
        CommitStickyModelFallback,
        Commit,
        CommitDeferredNames,
        CommitVisibilityFilter,
        CommsDrainExitedForUnregister,
        CompleteOp,
        CompleteUntilChangedSwitchTurnReconfigure,
        CompletionWaitersResolvedForUnregister,
        ConsumeInput,
        ConsumeOnAccept,
        DetachIngress,
        EnterExtraction,
        ExtractionFailed,
        ExtractionStart,
        ExtractionValidationFailed,
        ExtractionValidationPassed,
        Fail,
        FailOp,
        FatalFailure,
        ForceCancelNoRun,
        IncrementAttemptCount,
        InterruptCurrentRun,
        InteractionStreamAbandoned,
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
        PeerRequestSendFailed,
        PeerRequestSent,
        PeerRequestTimedOut,
        PeerResponseProgressArrived,
        PeerResponseReplied,
        PeerResponseTerminalArrived,
        Prepare,
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
        ClassifyRuntimeAuthorityReconciliation,
        RecoverRuntimeCompletionResultCorrelation,
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
        RuntimeLoopStoppedForUnregister,
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
        ReplaceVisibilityState,
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
        AuthorizeInteractionTerminalOutboxAdoption,
        AuthorizeStoredInputStateSeed,
        AuthorizeSupervisorMobPeerOverlay,
        BeginDeferredSessionArchive,
        BeginDeferredSessionPromotion,
        CancelSurfaceRequest,
        ClassifyAssistantOutput,
        ClassifyCallTimeout,
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
        ResolveVisibleRuntimePhase,
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

fn mob_spawn_command_plans() -> Vec<CommandPlanSchema> {
    let spawn_closure_lifecycle = || {
        vec![
            "Authorized".to_owned(),
            "Attempted".to_owned(),
            "Realized".to_owned(),
            "Failed".to_owned(),
            "Cancelled".to_owned(),
            "Abandoned".to_owned(),
        ]
    };
    let full_spawn_transitions = || {
        vec![
            transition_id("StageSpawnRunning"),
            transition_id("CompleteSpawnRunning"),
            transition_id("CompleteSpawnLateArrivalRunning"),
            transition_id("CompleteSpawnLateArrivalStopped"),
            transition_id("CompleteSpawnLateArrivalCompleted"),
            transition_id("CompleteSpawnDestroyed"),
            transition_id("CancelPendingSpawnPresentRunning"),
            transition_id("CancelPendingSpawnPresentStopped"),
            transition_id("CancelPendingSpawnPresentCompleted"),
            transition_id("CancelPendingSpawnAbsentRunning"),
            transition_id("CancelPendingSpawnAbsentStopped"),
            transition_id("CancelPendingSpawnAbsentCompleted"),
            transition_id("CancelPendingSpawnDestroyed"),
        ]
    };
    vec![
        CommandPlanSchema {
            name: "AuthorizedMobSpawnStart".to_owned(),
            authority_type: "PendingSpawnOperationOwnerAuthorized".to_owned(),
            source_inputs: vec![input_variant_id("CancelPendingSpawn")],
            source_signals: vec![
                signal_variant_id("StageSpawn"),
                signal_variant_id("CompleteSpawn"),
            ],
            transitions: full_spawn_transitions(),
            effects: vec![
                effect_variant_id("PendingSpawnOperationOwnerAuthorized"),
                effect_variant_id("ExposePendingSpawn"),
                effect_variant_id("EmitMemberLifecycleNotice"),
            ],
            effect_closures: vec![
                EffectClosureSchema {
                    effect: effect_variant_id("PendingSpawnOperationOwnerAuthorized"),
                    authority_type: "PendingSpawnOperationOwnerAuthorized".to_owned(),
                    closure_policy: "LocalPendingSpawnOwner".to_owned(),
                    lifecycle: spawn_closure_lifecycle(),
                },
                EffectClosureSchema {
                    effect: effect_variant_id("EmitMemberLifecycleNotice"),
                    authority_type: "CompleteSpawn".to_owned(),
                    closure_policy: "LocalSpawnCompletion".to_owned(),
                    lifecycle: spawn_closure_lifecycle(),
                },
            ],
        },
        CommandPlanSchema {
            name: "CanStartSpawn".to_owned(),
            authority_type: "CanStartSpawn".to_owned(),
            source_inputs: vec![],
            source_signals: vec![signal_variant_id("StageSpawn")],
            transitions: vec![transition_id("StageSpawnRunning")],
            effects: vec![effect_variant_id("PendingSpawnOperationOwnerAuthorized")],
            effect_closures: vec![EffectClosureSchema {
                effect: effect_variant_id("PendingSpawnOperationOwnerAuthorized"),
                authority_type: "CanStartSpawn".to_owned(),
                closure_policy: "LocalPendingSpawnOwner".to_owned(),
                lifecycle: spawn_closure_lifecycle(),
            }],
        },
        CommandPlanSchema {
            name: "SpawnStarted".to_owned(),
            authority_type: "SpawnStarted".to_owned(),
            source_inputs: vec![],
            source_signals: vec![signal_variant_id("StageSpawn")],
            transitions: vec![transition_id("StageSpawnRunning")],
            effects: vec![effect_variant_id("ExposePendingSpawn")],
            effect_closures: vec![EffectClosureSchema {
                effect: effect_variant_id("ExposePendingSpawn"),
                authority_type: "SpawnStarted".to_owned(),
                closure_policy: "LocalSpawnStarted".to_owned(),
                lifecycle: spawn_closure_lifecycle(),
            }],
        },
        CommandPlanSchema {
            name: "SpawnEffect".to_owned(),
            authority_type: "SpawnEffect".to_owned(),
            source_inputs: vec![input_variant_id("CancelPendingSpawn")],
            source_signals: vec![signal_variant_id("CompleteSpawn")],
            transitions: full_spawn_transitions(),
            effects: vec![effect_variant_id("EmitMemberLifecycleNotice")],
            effect_closures: vec![EffectClosureSchema {
                effect: effect_variant_id("EmitMemberLifecycleNotice"),
                authority_type: "SpawnEffect".to_owned(),
                closure_policy: "LocalSpawnCompletion".to_owned(),
                lifecycle: spawn_closure_lifecycle(),
            }],
        },
        CommandPlanSchema {
            name: "FailSpawn".to_owned(),
            authority_type: "FailSpawn".to_owned(),
            source_inputs: vec![input_variant_id("CancelPendingSpawn")],
            source_signals: vec![],
            transitions: vec![
                transition_id("CancelPendingSpawnPresentRunning"),
                transition_id("CancelPendingSpawnPresentStopped"),
                transition_id("CancelPendingSpawnPresentCompleted"),
                transition_id("CancelPendingSpawnAbsentRunning"),
                transition_id("CancelPendingSpawnAbsentStopped"),
                transition_id("CancelPendingSpawnAbsentCompleted"),
                transition_id("CancelPendingSpawnDestroyed"),
            ],
            effects: vec![effect_variant_id("EmitMemberLifecycleNotice")],
            effect_closures: vec![EffectClosureSchema {
                effect: effect_variant_id("EmitMemberLifecycleNotice"),
                authority_type: "FailSpawn".to_owned(),
                closure_policy: "LocalSpawnFailure".to_owned(),
                lifecycle: spawn_closure_lifecycle(),
            }],
        },
    ]
}

pub fn mob_machine_schema_metadata() -> MachineSchemaMetadata {
    let mut metadata = machine_schema_metadata(
        vec![
            // WAVE G2 machine folds (#14/#260/#261/#293/#314/#320): typed
            // verdict/fault/policy/capability/timeout classifiers owned by MobMachine.
            NamedTypeBinding::string_enum(
                "MemberAdmissionVerdictKind",
                &["Admitted", "DuplicateRejected"],
            ),
            NamedTypeBinding::string_enum("StepOutputFaultKind", &["MalformedJson"]),
            NamedTypeBinding::string_enum("StepFaultDispositionKind", &["Retry", "Terminal"]),
            NamedTypeBinding::string_enum(
                "SupervisorEscalationFailureCause",
                &["NoEligibleSupervisor", "TimeoutExceeded"],
            ),
            NamedTypeBinding::string_enum("PolicyDecision", &["Allow", "Deny"]),
            NamedTypeBinding::string_enum(
                "ExternalMemberRebindCapability",
                &["Unavailable", "Available"],
            ),
            NamedTypeBinding::string_enum(
                "TurnTimeoutDisposition",
                &["Detached", "Canceled", "Retryable"],
            ),
            NamedTypeBinding::string_enum(
                "IdentityAuthorityCondition",
                &[
                    "Unavailable",
                    "Missing",
                    "Malformed",
                    "PresentCreateIfAbsent",
                    "PresentRequireExisting",
                    "Absent",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityLeaseCondition",
                &[
                    "Unavailable",
                    "Missing",
                    "Malformed",
                    "HeldByCurrentIncarnation",
                    "HeldByOtherLiveIncarnation",
                    "HeldByExpiredIncarnation",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityResourceCondition",
                &[
                    "Unavailable",
                    "Missing",
                    "Matching",
                    "Divergent",
                    "Malformed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentitySessionCondition",
                &[
                    "Unavailable",
                    "Missing",
                    "Matching",
                    "RecoverableDivergence",
                    "AmbiguousDivergence",
                    "Malformed",
                    "IrrecoverablyCorrupt",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityReceiptCondition",
                &[
                    "NotRequired",
                    "Unavailable",
                    "Missing",
                    "Matching",
                    "Conflicting",
                    "Malformed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityExternalTrustCondition",
                &[
                    "NotRequired",
                    "Unavailable",
                    "Matching",
                    "Absent",
                    "Contradictory",
                    "Indeterminate",
                    "Malformed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityExternalCeremonyCondition",
                &[
                    "NotRequired",
                    "FreshAvailable",
                    "TemporarilyUnavailable",
                    "AwaitFresh",
                    "SpentOrUnknown",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityInitialDeliveryCondition",
                &[
                    "NotRequired",
                    "Unavailable",
                    "ProvenAbsent",
                    "AcceptedPendingExact",
                    "CommittedExact",
                    "ContentOnlyMatch",
                    "OperationCollision",
                    "Contradictory",
                    "Indeterminate",
                    "Malformed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "IdentityReconcileDecision",
                &[
                    "Backoff",
                    "RepairBlocked",
                    "AcquireLease",
                    "AwaitLease",
                    "SealRetirementProven",
                    "SealSessionCreationConsumed",
                    "EnsureSessionAuthority",
                    "EnsureRuntimeRegistration",
                    "AwaitExternalBindingCeremony",
                    "EnsureExternalBindingReceipt",
                    "EnsureExternalBinding",
                    "EnsureMemberMaterialization",
                    "EnsureInitialDeliveryReceipt",
                    "EnsureInitialDelivery",
                    "AwaitInitialDelivery",
                    "ReconcileWiring",
                    "RetireMemberMaterialization",
                    "RetireRuntimeRegistration",
                    "ReleaseSessionAuthority",
                    "Converged",
                    "Tombstoned",
                    "Quarantined",
                ],
            ),
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
            NamedTypeBinding::string("AdaptiveRunId"),
            NamedTypeBinding::string("AdaptiveLayerId"),
            NamedTypeBinding::string_enum(
                "AdaptiveRunPhase",
                &[
                    "Active",
                    "CleanupRequired",
                    "EvidenceMissing",
                    "Finished",
                    "Failed",
                    "Canceled",
                ],
            ),
            NamedTypeBinding::string_enum(
                "AdaptiveStopReason",
                &[
                    "FinishDecision",
                    "DepthLimit",
                    "PlanLimit",
                    "RepairLimit",
                    "FailureLimit",
                    "BudgetExhausted",
                    "DeadlineExceeded",
                    "HostCancel",
                ],
            ),
            NamedTypeBinding::string_enum("AdaptiveDecisionKind", &["RunLayer", "Finish"]),
            NamedTypeBinding::string_enum(
                "AdaptiveLayerPhase",
                &[
                    "Validating",
                    "Admitted",
                    "Provisioning",
                    "Running",
                    "Collecting",
                    "Completed",
                    "SetupFailed",
                    "RunFailed",
                    "ResultInvalid",
                    "Canceled",
                ],
            ),
            NamedTypeBinding::string_enum("AdaptiveLayerAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum(
                "AdaptiveLayerSetupFaultKind",
                &[
                    "MobCreateFailed",
                    "SpawnFailed",
                    "WiringFailed",
                    "CanceledDuringSetup",
                    "Interrupted",
                ],
            ),
            NamedTypeBinding::string_enum(
                "AdaptiveLayerDispositionKind",
                &["Destroyed", "Retained", "RetainedAsEvidence"],
            ),
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
                "SpawnExecPhase",
                &[
                    "Opened",
                    "MaterializePending",
                    "MembershipCommitted",
                    "Activated",
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
                    "Stopped",
                    "Resumed",
                    "Destroying",
                    "DestroyStorageFinalizing",
                    "MemberSpawned",
                    "MemberRetirementStartedReleasing",
                    "MemberRetirementStartedPreservingBinding",
                    "MemberRetirementStartedPeerOnly",
                    "RemoteMemberRuntimeRetired",
                    "RemoteMemberSupervisorRevoked",
                    "MemberRetired",
                    "RespawnTopologyAbandoned",
                    "Reset",
                    "MemberRetirementStarted",
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
            NamedTypeBinding::string_enum(
                "MobSpawnMemberAdmissionKind",
                &[
                    "Denied",
                    "Allowed",
                    "NonPortableRustBundles",
                    "NonPortablePerSpawnExternalTools",
                    "NonPortableMobDefaultExternalTools",
                    "NonPortableDefaultLlmClientOverride",
                    "NonPortableHostSurfaceMcpAllowlist",
                    "NonPortableInheritedToolFilter",
                    "NonPortableWorkgraphTools",
                    "SecretBearingShellEnv",
                    "SecretBearingMcpStdioEnv",
                    "SecretBearingMcpHttpHeaders",
                    "MissingHostCapabilityAutonomousMembers",
                    "MissingHostCapabilityDurableSessions",
                    "MissingHostCapabilityTrackedInputCancel",
                    "MissingHostCapabilityProtocolV4",
                    "MissingHostCapabilityMemoryStore",
                    "MissingHostCapabilityMcp",
                    "HostNotBound",
                    "OwnerBridgeSessionAbsent",
                    "LaunchModePlacementMismatch",
                    "ResolvedSpecDigestAbsent",
                ],
            ),
            NamedTypeBinding::string_enum("MobCurrentMobAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum("MobSpawnToolAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum("MobCreateMobAdmissionKind", &["Denied", "Allowed"]),
            NamedTypeBinding::string_enum(
                "MobProfileMutationAdmissionKind",
                &["Denied", "Allowed"],
            ),
            NamedTypeBinding::string_enum(
                "MobMemberOperationEligibilityKind",
                &["DeniedNotRunning", "Admitted"],
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
                "MemberLiveMaterializationObservationKind",
                &["DurableSnapshotPresent", "DurableSnapshotMissing"],
            ),
            NamedTypeBinding::string_enum(
                "MemberRevivalVerdictKind",
                &["ReviveAuthorized", "BrokenRecorded"],
            ),
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
                "AutonomousShutdownMemberActionKind",
                &["Interrupt", "SkipTerminalRetirementAnchor"],
            ),
            NamedTypeBinding::string_enum(
                "MemberWaitClassificationKind",
                &["RuntimeMaterialPresent", "MissingRuntimeMaterial"],
            ),
            NamedTypeBinding::string_enum(
                "MemberHealthClass",
                &["Healthy", "Degraded", "Wedged", "Unknown"],
            ),
            NamedTypeBinding::string_enum(
                "MemberProgressEventKind",
                &["ExecutionAdvanced", "BecameIdle", "Unchanged"],
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
            // Multi-host mobs (§6.1/§6.2/§6.5/§15/§18) named types.
            NamedTypeBinding::type_path("HostId", "crate::catalog::dsl::mob_machine::HostId"),
            NamedTypeBinding::u64("HostBindingGeneration"),
            NamedTypeBinding::type_path_struct(
                "HostBindingGenerationTombstone",
                "crate::catalog::dsl::mob_machine::HostBindingGenerationTombstone",
                vec![
                    TypePathStructField::named("host_id", "HostId"),
                    TypePathStructField::named("binding_generation", "HostBindingGeneration"),
                ],
            ),
            NamedTypeBinding::string("PlacedSpawnId"),
            NamedTypeBinding::string_enum(
                "PlacedSpawnCarrierExpectedPhase",
                &["Pending", "Committed"],
            ),
            NamedTypeBinding::type_path_struct(
                "PlacedCarrierCleanupObligation",
                "crate::catalog::dsl::mob_machine::PlacedCarrierCleanupObligation",
                vec![
                    TypePathStructField::named("agent_identity", "AgentIdentity"),
                    TypePathStructField::named("spawn_id", "PlacedSpawnId"),
                    TypePathStructField::named("generation", "Generation"),
                    TypePathStructField::named("fence_token", "FenceToken"),
                    TypePathStructField::string("provision_operation_id"),
                    TypePathStructField::named("operation_owner_session_id", "SessionId"),
                    TypePathStructField::named("expected_phase", "PlacedSpawnCarrierExpectedPhase"),
                ],
            ),
            NamedTypeBinding::string_enum("HostBindPhase", &["Requested", "Bound"]),
            NamedTypeBinding::type_path(
                "LiveWsEndpointUrl",
                "crate::catalog::dsl::mob_machine::LiveWsEndpointUrl",
            ),
            NamedTypeBinding::type_path(
                "PrincipalId",
                "crate::catalog::dsl::mob_machine::PrincipalId",
            ),
            // String binding (not type_path): MeerkatMachine already declares
            // `InputId` as String and the meerkat_mob_seam composition requires
            // one binding per name — it is the same delivery-input-id fact
            // (SessionId follows the same newtype-in-tail/string-in-metadata
            // pattern).
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string_enum(
                "ControlScope",
                &[
                    "List",
                    "ReadHistory",
                    "SubscribeEvents",
                    "SendCommand",
                    "Cancel",
                    "Retire",
                    "WireTopology",
                    "Live",
                    "AdminHost",
                    "AdminGrants",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MemberSessionDisposal",
                &[
                    "Archived",
                    "RuntimeReleasedOnlyHostOwned",
                    "RuntimeReleasedOnlyNoDurableSessions",
                ],
            ),
            NamedTypeBinding::string_enum(
                "FlowStepDispatchKind",
                &[
                    "Local",
                    "RemoteTurnDirective",
                    "RejectedOverlayAutonomous",
                    "RejectedHostIncapable",
                ],
            ),
            NamedTypeBinding::string_enum(
                "MemberOperatorRejectKind",
                &[
                    "UnknownIdentity",
                    "SenderKeyMismatch",
                    "StaleGeneration",
                    "StaleFence",
                    "StaleSession",
                    "StaleHost",
                    "StaleHostBindingGeneration",
                    "HostRevoked",
                    "NoPlacement",
                ],
            ),
            NamedTypeBinding::string_enum("RouteObligationKind", &["Install", "Remove"]),
            NamedTypeBinding::u64("RemoteTurnDispatchSequence"),
            NamedTypeBinding::type_path_struct(
                "RouteInstallObligation",
                "crate::catalog::dsl::mob_machine::RouteInstallObligation",
                vec![
                    TypePathStructField::named("edge", "WiringEdge"),
                    TypePathStructField::named("host", "HostId"),
                    TypePathStructField::named("kind", "RouteObligationKind"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "RemoteTurnObligation",
                "crate::catalog::dsl::mob_machine::RemoteTurnObligation",
                vec![
                    TypePathStructField::named("agent_identity", "AgentIdentity"),
                    TypePathStructField::named("host_id", "HostId"),
                    TypePathStructField::named("host_binding_generation", "HostBindingGeneration"),
                    TypePathStructField::named("member_session_id", "SessionId"),
                    TypePathStructField::named("generation", "Generation"),
                    TypePathStructField::named("fence_token", "FenceToken"),
                    TypePathStructField::named("dispatch_sequence", "RemoteTurnDispatchSequence"),
                    TypePathStructField::named("input_id", "InputId"),
                    TypePathStructField::named("run_id", "RunId"),
                    TypePathStructField::named("step_id", "StepId"),
                ],
            ),
            NamedTypeBinding::string_enum(
                "PlacedCompletionLifecycleIntentKind",
                &["Stop", "Reset", "Complete", "RetireAll", "Destroy"],
            ),
            NamedTypeBinding::type_path_struct(
                "PlacedCompletionObligation",
                "crate::catalog::dsl::mob_machine::PlacedCompletionObligation",
                vec![
                    TypePathStructField::named("agent_identity", "AgentIdentity"),
                    TypePathStructField::named("host_id", "HostId"),
                    TypePathStructField::named("host_binding_generation", "HostBindingGeneration"),
                    TypePathStructField::named("member_session_id", "SessionId"),
                    TypePathStructField::named("generation", "Generation"),
                    TypePathStructField::named("fence_token", "FenceToken"),
                    TypePathStructField::named("dispatch_sequence", "RemoteTurnDispatchSequence"),
                    TypePathStructField::named("input_id", "InputId"),
                ],
            ),
            NamedTypeBinding::type_path_struct(
                "PlacedKickoffObligation",
                "crate::catalog::dsl::mob_machine::PlacedKickoffObligation",
                vec![
                    TypePathStructField::named("agent_identity", "AgentIdentity"),
                    TypePathStructField::named("host_id", "HostId"),
                    TypePathStructField::named("host_binding_generation", "HostBindingGeneration"),
                    TypePathStructField::named("member_session_id", "SessionId"),
                    TypePathStructField::named("generation", "Generation"),
                    TypePathStructField::named("fence_token", "FenceToken"),
                    TypePathStructField::named("input_id", "InputId"),
                    TypePathStructField::string("objective_id"),
                ],
            ),
            NamedTypeBinding::string_enum(
                "PlacedKickoffOutcomeKind",
                &[
                    "Started",
                    "CallbackPending",
                    "Failed",
                    "Cancelled",
                    "RejectedNoEffect",
                    "Disposed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "PlacedKickoffClosureKind",
                &["Acknowledged", "Disposed", "RejectedNoEffect"],
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
    // Identity classification is a stateless pure projection. TLC proves its
    // lifecycle integration with one typed payload representative per field;
    // the generated-kernel product tests exhaustively prove all classifier
    // payload combinations and decision mapping.
    .with_tlc_representative_input(input_variant_id("ClassifyIdentityReconciliation"))
    // ADJ-P4-5: the plan's 2 hosts x 3 members deep-verification bound —
    // deep-profile model-checker configuration, not machine vocabulary.
    .with_deep_domain_override("AgentIdentityValues", 3);
    metadata.command_plans = mob_spawn_command_plans();
    metadata
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
        // Spawn-exec ladder internal sub-steps driven by finalize_spawn_*; the
        // membership-establishing `CommitSpawnMembership` is the surfaced spawn
        // command target and is NOT runtime-internal.
        BeginSpawnExec,
        CommitSpawnActivation,
        AbortSpawnExec,
        // Exact placed-carrier deletion is a runtime-owned recovery/teardown
        // protocol. Public commands cannot authorize or resolve the persisted
        // operation/phase witness directly.
        AuthorizePlacedCarrierCleanup,
        ResolvePlacedCarrierCleanup,
        ClassifyFlowRunTerminality,
        ClassifyFlowStepTerminality,
        ClassifyFlowFrameTerminalStatus,
        ClassifyFlowRunPublicResult,
        BindOwnerBridgeSession,
        ClassifyMemberWait,
        ResolveAutonomousShutdownMemberAction,
        // Sampled by the actor as part of `MemberStatus`; the transition owns
        // the durable progress/health projection but is not independently
        // callable from a public surface.
        ObserveMemberProgress,
        ClassifySpawnManyFailure,
        ResolveFlowDelegationEdgeAdmission,
        ClassifyRemoteMemberRuntimeObservation,
        ResolveSpawnMemberAdmission,
        ResolveCurrentMobAdmission,
        ResolveSpawnToolAdmission,
        ResolveCreateMobAdmission,
        ResolveProfileMutationAdmission,
        ClassifyMemberOperationEligibility,
        ClassifyRetirePendingSpawnDisposition,
        ClassifyBridgeRejectionRecovery,
        ClassifyPendingSupervisorAcceptance,
        ClassifyIdentityReconciliation,
        CreateFrameSeed,
        CreateLoopSeed,
        CreateRunSeed,
        KickoffCancelRequested,
        KickoffClear,
        KickoffMarkPending,
        BindObjectiveOwner,
        KickoffMarkStarting,
        // 0.7.2 L5 disciplined shell inputs: teardown drain-obligation
        // closures fired by the mob actor, never by surfaces.
        KickoffQuiesced,
        CancelPendingSpawn,
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
        // Generated composition failure closure inputs. These are emitted by
        // the actor shell after a routed runtime consumer refusal and are not
        // callable through the public mob command surface.
        ResolveRuntimeBindingRefusal,
        ResolveRuntimeIngressRefusal,
        ResolveRuntimeRetireRefusal,
        RetryRuntimeRetire,
        RecordRemoteMemberRuntimeRetired,
        RecordRemoteMemberSupervisorRevoked,
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
        AuthorizeMemberEndpointMigrationTrustCleanup,
        AuthorizeRetiringMemberPeerOverlayCleanup,
        AuthorizeRetiringMemberTrustCleanupObserved,
        CleanupRetiringMemberWiring,
        RestoreRetiringMemberWiring,
        CleanupRetiringExternalPeer,
        RestoreRetiringExternalPeer,
        CleanupRetiringExternalPeerObservedAbsent,
        RestoreRetiringExternalPeerObservedAbsent,
        AuthorizeExternalPeerReciprocalTrust,
        AdmitSupervisorRotation,
        ProvisionSupervisorAuthority,
        RecordSupervisorPendingRotation,
        CommitSupervisorRotation,
        ClearSupervisorAuthorityForDestroy,
        RestoreSupervisorAuthorityAfterDestroyRollback,
        RecordPendingRecipientTrust,
        ResolvePendingRecipientTrust,
        RollbackPendingRecipientTrust,
        // Multi-host mobs (§6/§15/§18): host bind lifecycle, materialization
        // failure recording, route/turn obligations, grants, member-operator
        // upcall admission, and flow-step dispatch classification are all
        // driven by the runtime shell in phase 1 (surfaces land later).
        BeginHostBind,
        CommitHostBind,
        HostRebound,
        PromoteCommittedPlacedSpawnCarrierBinding,
        RefreshHostCapabilities,
        RevokeHost,
        RecordMemberMaterializationFailure,
        RecordRouteInstall,
        AuthorizeRouteRemovalBeforeUnwire,
        ResolveRouteInstall,
        RollbackRouteInstall,
        RecordRemoteTurnObligation,
        AbortRemoteTurnObligation,
        CommitRemoteTurnOutcome,
        ResolveRemoteTurnObligation,
        AcknowledgeRemoteTurnOutcome,
        DisposeRemoteTurnObligation,
        RecordPlacedCompletionObligation,
        RequestPlacedCompletionCancellation,
        ResolvePlacedCompletionOutcome,
        ClosePlacedCompletionOutcome,
        AcknowledgePlacedCompletionOutcome,
        DisposePlacedCompletionOutcome,
        BeginPlacedCompletionLifecycleQuiesce,
        EndPlacedCompletionLifecycleQuiesce,
        StartPlacedKickoff,
        ResolvePlacedKickoffStarted,
        ResolvePlacedKickoffCallbackPending,
        ResolvePlacedKickoffFailed,
        ResolvePlacedKickoffCancelled,
        RejectPlacedKickoffBeforeAdmission,
        AcknowledgePlacedKickoffOutcome,
        DisposePlacedKickoffObligation,
        GrantOperatorScopes,
        RevokeOperatorScopes,
        ResolveMemberOperatorAdmission,
        ClassifyFlowStepDispatch,
        RecordCoordinationWorkIntent,
        RecordCoordinationResourceClaim,
        UpdateCoordinationWorkIntentStatus,
        UpdateCoordinationResourceClaimStatus,
        ObserveCoordinationResourceClaimOverlap,
        InitializeAdaptiveRun,
        RecordPlanningDecision,
        RecordPlanRejected,
        ResolveLayerAdmission,
        RecordLayerProvisioned,
        RecordLayerRunStarted,
        IngestLayerTerminal,
        RecordLayerSetupFault,
        RecordLayerInterrupted,
        RecordLayerResultValidated,
        RecordLayerResultInvalid,
        RecordLayerMobDestroyed,
        RecordLayerMobRetained,
        RecordCleanupResolved,
        RecordBodyEvidenceMissing,
        ResolveAdaptiveFinish,
        RequestAdaptiveCancel,
        RecordDeadlineObserved,
        // WAVE G2 machine folds: shell-driven classifier + seed inputs.
        ProbeMemberAdmission,
        ComputeRespawnGeneration,
        ClassifyStepOutputFault,
        EscalateToSupervisor,
        EscalateToSupervisorNoEligibleTarget,
        EvaluateTopologyEdge,
        SetExternalMemberRebindCapability,
        ClassifyTurnTimeoutDisposition,
        SeedOrphanBudget,
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
            // WAVE G1 fold (#250): opaque typed target-binding identity newtype.
            NamedTypeBinding::string("TargetBindingId"),
            // Typed trigger identity (#18): distinct from TargetBindingId —
            // trigger identity and target-binding identity are different
            // semantic facts.
            NamedTypeBinding::string("TriggerKey"),
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
            // Typed semantic handles (#18): trigger identity, target-binding
            // identity, claim ownership, and delivery correlation are
            // string-backed newtypes, never raw `String`s the machine could
            // confuse with policy keys or free-form detail text.
            NamedTypeBinding::string("TriggerKey"),
            NamedTypeBinding::string("TargetBindingId"),
            NamedTypeBinding::string("ClaimOwner"),
            NamedTypeBinding::string("CorrelationId"),
            // Dogma K8: runtime-outcome receipt key is a typed identity, not
            // a raw String in canonical machine state.
            NamedTypeBinding::string("RuntimeOutcomeKey"),
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
                // 0.7.2 D2a: AlreadySuperseded keeps the post-completion
                // classification total when the supersession sweep landed
                // between dispatch and completion.
                &["Supersede", "Proceed", "AlreadySuperseded"],
            ),
            // 0.7.2 D2a: typed class of a delivery resolution that arrived
            // after the occurrence was superseded.
            NamedTypeBinding::string_enum(
                "LateCompletionResolutionClass",
                &[
                    "DeliveryCompleted",
                    "RuntimeCompleted",
                    "RuntimeRejected",
                    "RuntimeTransportError",
                    "RuntimeInternalError",
                    "DeliveryCompletionFailed",
                    "DeliveryFailed",
                ],
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
                    "ClassifyStaleCompletionArrival",
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
                    "StaleCompletionArrivalClassificationRejected",
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
    .with_ci_step_limit(3)
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
                "WorkCreateCompletionPolicyAdmissionKind",
                &["DeniedNonSelfAttest", "Admitted"],
            ),
            NamedTypeBinding::string_enum(
                "WorkCloseStatusAdmissionKind",
                &[
                    "DeniedNonTerminal",
                    "AdmittedCompleted",
                    "AdmittedCancelled",
                    "AdmittedFailed",
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkPublicConfirmationAdmissionKind",
                &["DeniedRequiresTrustedHost", "Admitted"],
            ),
            NamedTypeBinding::string_enum(
                "WorkCompletionPolicyMutationAdmissionKind",
                &["Denied", "Admitted"],
            ),
            NamedTypeBinding::string_enum(
                "WorkPolicyEscalationAdmissionKind",
                &["Denied", "Admitted"],
            ),
            NamedTypeBinding::string_enum(
                "WorkConfirmationEvidenceObservation",
                &[
                    "Empty",
                    "Other",
                    "HostConfirmation",
                    "PrincipalConfirmation",
                    "SupervisorConfirmation",
                    "ReviewerConfirmation",
                ],
            ),
            NamedTypeBinding::string_enum(
                "WorkConfirmationAdmissionKind",
                &[
                    "DeniedPrincipalRequired",
                    "DeniedPrincipalKindMismatch",
                    "DeniedSupervisorMismatch",
                    "DeniedEvidenceKind",
                    "DeniedSelfAttestEmptyEvidenceKind",
                    "Admitted",
                ],
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
    .with_ci_step_limit(5)
}
