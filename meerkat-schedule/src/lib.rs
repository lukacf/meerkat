#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
pub use ::tokio;

mod driver;
mod error;
mod lifecycle;
pub(crate) mod machines;
mod service;
mod store;
mod surface;
mod tool_surface;
mod tools;
mod trigger;
mod types;

pub use driver::{
    DeliveryCompletion, DeliveryDispatch, DeliveryTerminal, ScheduleDriver, ScheduleDriverConfig,
    ScheduleTargetDelivery, ScheduleTargetProbe, ScheduleTickReport, TargetProbeOutcome,
};
pub use error::{ScheduleDomainError, ScheduleStoreError};
pub use lifecycle::{
    AuthorizedOccurrenceWrite, AuthorizedScheduleWrite, ClaimedDispatchDisposition,
    ClaimedDispatchVerdict, CompletionSupersessionDisposition, CompletionSupersessionVerdict,
    OccurrenceDueAction, OccurrenceLifecycleEffect, OccurrenceLifecycleError,
    OccurrenceLifecycleInput, OccurrenceLifecycleMutator, OccurrenceSupersessionAck,
    OccurrenceWritePrecondition, ScheduleLifecycleEffect, ScheduleLifecycleError,
    ScheduleLifecycleInput, ScheduleLifecycleMutator, ScheduleWritePrecondition,
};
pub use service::ScheduleService;
pub use store::{
    ClaimDueRequest, ClaimDueResult, DisabledScheduleStore, MemoryScheduleStore, OccurrenceFilter,
    PendingSupersession, ScheduleFilter, ScheduleStore, ScheduleStoreKind,
    apply_supersession_feedback,
};
pub use surface::wire_schedule_tools;
pub use tool_surface::ScheduleToolSurface;
pub use tools::{
    CAPABILITY_UNAVAILABLE as SCHEDULE_TOOL_CAPABILITY_UNAVAILABLE,
    CurrentSessionScheduleToolDispatcher, INVALID_ARGUMENTS as SCHEDULE_TOOL_INVALID_ARGUMENTS,
    NOT_FOUND as SCHEDULE_TOOL_NOT_FOUND, ScheduleToolDispatcher, ScheduleToolError,
    handle_schedule_tools_call, schedule_tools_list,
};
pub use trigger::{CronAuthoringSpec, next_due_after, occurrences_for_horizon};
pub use types::{
    CalendarFieldSpec, CalendarTriggerSpec, CreateScheduleRequest, DeliveryCompletionFailureReason,
    DeliveryFailureReason, DeliveryReceipt, DeliveryReceiptStage, ForkContextSpec,
    HelperOptionsSpec, IntervalTriggerSpec, MisfirePolicy, MissingTargetPolicy, MobTargetBinding,
    Occurrence, OccurrenceFailureClass, OccurrenceId, OccurrenceOrdinal, OccurrencePhase,
    OccurrenceTargetProbeOutcome, OverlapPolicy, ResolvedSpawnSnapshot, RuntimeCompletionOutcome,
    RuntimeDeliveryOutcome, Schedule, ScheduleConfig, ScheduleId, SchedulePhase, ScheduleRevision,
    ScheduleSpawnTooling, ScheduledMobAction, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
    ScheduledSessionAction, SessionMaterializationSpec, SessionTargetBinding, TargetBinding,
    TriggerSpec, UpdateScheduleRequest,
};

pub const SCHEDULE_CAPABILITY_DISABLED_DESCRIPTION: &str = "config.tools.schedule_enabled is false";

pub fn schedule_capability_enabled(config: &meerkat_core::Config) -> bool {
    config.tools.schedule_enabled
}

pub const SCHEDULE_CAPABILITY_POLICY: meerkat_capabilities::FeatureCapabilityPolicy =
    meerkat_capabilities::FeatureCapabilityPolicy::new(
        schedule_capability_enabled,
        SCHEDULE_CAPABILITY_DISABLED_DESCRIPTION,
    );

pub const fn schedule_capability_policy() -> meerkat_capabilities::FeatureCapabilityPolicy {
    SCHEDULE_CAPABILITY_POLICY
}

inventory::submit! {
    meerkat_capabilities::CapabilityRegistration {
        id: meerkat_capabilities::CapabilityId::Schedule,
        description: "Realm-scoped durable schedules and occurrence delivery",
        scope: meerkat_capabilities::CapabilityScope::Universal,
        requires_feature: None,
        prerequisites: &[],
        status_resolver: Some(|config| {
            let policy = crate::schedule_capability_policy();
            if policy.is_enabled(config) {
                meerkat_capabilities::CapabilityStatus::Available
            } else {
                meerkat_capabilities::CapabilityStatus::DisabledByPolicy {
                    description: policy.disabled_description().into(),
                }
            }
        }),
    }
}

#[cfg(feature = "skills")]
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "schedule-workflow",
        name: "Schedule Workflow",
        description: "How to author and inspect durable schedules from agent tools",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["schedule"],
        body: include_str!("../skills/schedule-workflow/SKILL.md"),
        extensions: &[],
    }
}

#[doc(hidden)]
#[cfg(feature = "machine-schema-exports")]
pub mod machine_schema_exports {
    pub fn schedule_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::schedule_lifecycle_schema_metadata()
            .attach_to(crate::machines::schedule_lifecycle::ScheduleLifecycleMachineState::schema())
    }

    pub fn occurrence_lifecycle_schema() -> meerkat_machine_schema::MachineSchema {
        meerkat_machine_schema::catalog::dsl::occurrence_lifecycle_schema_metadata().attach_to(
            crate::machines::occurrence_lifecycle::OccurrenceLifecycleMachineState::schema(),
        )
    }
}
