use chrono::{DateTime, Utc};
use meerkat_core::types::RenderMetadata;
use meerkat_core::{ContentInput, OutputSchema, PeerMeta, Provider, SessionId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const DEFAULT_PLANNING_HORIZON_DAYS: u32 = 30;
const DEFAULT_PLANNING_HORIZON_OCCURRENCES: u32 = 64;

macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(
            #[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid
        );

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            pub fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            pub fn parse(value: &str) -> Result<Self, uuid::Error> {
                Ok(Self(Uuid::parse_str(value)?))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }
    };
}

uuid_newtype!(
    /// Opaque identifier for a persisted schedule.
    ScheduleId
);

uuid_newtype!(
    /// Opaque identifier for a persisted occurrence.
    OccurrenceId
);

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScheduleRevision(pub u64);

impl ScheduleRevision {
    pub fn initial() -> Self {
        Self(1)
    }

    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OccurrenceOrdinal(pub u64);

impl OccurrenceOrdinal {
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulePhase {
    Active,
    Paused,
    Deleted,
}

impl SchedulePhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Deleted)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrencePhase {
    Pending,
    Claimed,
    Dispatching,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    DeliveryFailed,
}

impl OccurrencePhase {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Skipped
                | Self::Misfired
                | Self::Superseded
                | Self::DeliveryFailed
        )
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TriggerSpec {
    Once { due_at_utc: DateTime<Utc> },
    Interval(IntervalTriggerSpec),
    Calendar(CalendarTriggerSpec),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntervalTriggerSpec {
    pub start_at_utc: DateTime<Utc>,
    pub every_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_at_utc: Option<DateTime<Utc>>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum CalendarFieldSpec {
    Any,
    Values(Vec<u32>),
}

impl CalendarFieldSpec {
    pub fn contains(&self, value: u32) -> bool {
        match self {
            Self::Any => true,
            Self::Values(values) => values.contains(&value),
        }
    }

    pub fn normalized(mut values: Vec<u32>) -> Self {
        values.sort_unstable();
        values.dedup();
        Self::Values(values)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarTriggerSpec {
    pub timezone: String,
    #[serde(default)]
    pub minute: CalendarFieldSpec,
    #[serde(default)]
    pub hour: CalendarFieldSpec,
    #[serde(default)]
    pub day_of_month: CalendarFieldSpec,
    #[serde(default)]
    pub month: CalendarFieldSpec,
    #[serde(default)]
    pub day_of_week: CalendarFieldSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<CalendarFieldSpec>,
}

impl Default for CalendarFieldSpec {
    fn default() -> Self {
        Self::Any
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MisfirePolicy {
    Skip,
    CatchUpWithin { window_seconds: u64 },
}

impl Default for MisfirePolicy {
    fn default() -> Self {
        Self::Skip
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    AllowConcurrent,
    SkipIfRunning,
}

impl Default for OverlapPolicy {
    fn default() -> Self {
        Self::SkipIfRunning
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingTargetPolicy {
    Skip,
    MarkMisfired,
}

impl Default for MissingTargetPolicy {
    fn default() -> Self {
        Self::MarkMisfired
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "target_kind", rename_all = "snake_case")]
pub enum TargetBinding {
    Session(SessionTargetBinding),
    Mob(MobTargetBinding),
}

impl TargetBinding {
    pub fn stable_key(&self) -> String {
        match self {
            Self::Session(binding) => binding.stable_key(),
            Self::Mob(binding) => binding.stable_key(),
        }
    }

    pub fn bind_materialized_session(&mut self, session_id: &SessionId) -> bool {
        match self {
            Self::Session(binding) => binding.bind_materialized_session(session_id),
            Self::Mob(_) => false,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionTargetBinding {
    ExactSession {
        session_id: SessionId,
        action: ScheduledSessionAction,
    },
    ResumableSession {
        session_id: SessionId,
        action: ScheduledSessionAction,
    },
    MaterializeOnDemandSession {
        create: SessionMaterializationSpec,
        action: ScheduledSessionAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bound_session_id: Option<SessionId>,
    },
}

impl SessionTargetBinding {
    pub fn stable_key(&self) -> String {
        match self {
            Self::ExactSession { session_id, .. } => format!("session:exact:{session_id}"),
            Self::ResumableSession { session_id, .. } => {
                format!("session:resumable:{session_id}")
            }
            Self::MaterializeOnDemandSession {
                create,
                bound_session_id,
                ..
            } => bound_session_id.as_ref().map_or_else(
                || {
                    let name = create
                        .comms_name
                        .clone()
                        .unwrap_or_else(|| create.model.clone());
                    format!("session:materialize:{name}")
                },
                |session_id| format!("session:materialize:{session_id}"),
            ),
        }
    }

    pub fn action(&self) -> &ScheduledSessionAction {
        match self {
            Self::ExactSession { action, .. }
            | Self::ResumableSession { action, .. }
            | Self::MaterializeOnDemandSession { action, .. } => action,
        }
    }

    pub fn resolved_session_id(&self) -> Option<&SessionId> {
        match self {
            Self::ExactSession { session_id, .. } | Self::ResumableSession { session_id, .. } => {
                Some(session_id)
            }
            Self::MaterializeOnDemandSession {
                bound_session_id, ..
            } => bound_session_id.as_ref(),
        }
    }

    pub fn bind_materialized_session(&mut self, session_id: &SessionId) -> bool {
        match self {
            Self::MaterializeOnDemandSession {
                bound_session_id, ..
            } => match bound_session_id {
                Some(existing) if existing != session_id => false,
                Some(_) => false,
                None => {
                    *bound_session_id = Some(session_id.clone());
                    true
                }
            },
            Self::ExactSession { .. } | Self::ResumableSession { .. } => false,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledSessionAction {
    Prompt {
        prompt: ContentInput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        skill_references: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        additional_instructions: Vec<String>,
    },
    Event {
        event_type: String,
        payload: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMaterializationSpec {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema_json: Option<serde_json::Value>,
    #[serde(default)]
    pub structured_output_retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comms_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_meta: Option<PeerMeta>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preload_skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_generation: Option<u64>,
    #[serde(default)]
    pub keep_alive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_context: Option<serde_json::Value>,
}

impl SessionMaterializationSpec {
    pub fn with_output_schema(mut self, output_schema: OutputSchema) -> Self {
        self.output_schema_json = serde_json::to_value(output_schema).ok();
        self
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MobTargetBinding {
    Member {
        mob_id: String,
        member_id: String,
        action: ScheduledMobAction,
    },
    Flow {
        mob_id: String,
        flow_id: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    SpawnHelper {
        mob_id: String,
        member_id: String,
        prompt: String,
        #[serde(default)]
        options: HelperOptionsSpec,
    },
    ForkHelper {
        mob_id: String,
        source_member_id: String,
        member_id: String,
        prompt: String,
        #[serde(default)]
        fork_context: ForkContextSpec,
        #[serde(default)]
        options: HelperOptionsSpec,
    },
}

impl MobTargetBinding {
    pub fn stable_key(&self) -> String {
        match self {
            Self::Member {
                mob_id, member_id, ..
            } => format!("mob:{mob_id}:member:{member_id}"),
            Self::Flow {
                mob_id, flow_id, ..
            } => format!("mob:{mob_id}:flow:{flow_id}"),
            Self::SpawnHelper {
                mob_id, member_id, ..
            } => format!("mob:{mob_id}:spawn_helper:{member_id}"),
            Self::ForkHelper {
                mob_id,
                source_member_id,
                member_id,
                ..
            } => format!("mob:{mob_id}:fork_helper:{source_member_id}:{member_id}"),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduledMobAction {
    Send {
        content: ContentInput,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        render_metadata: Option<RenderMetadata>,
    },
}

pub type MobActionSpec = ScheduledMobAction;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HelperOptionsSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<ScheduledMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<ScheduledMobBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<serde_json::Value>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledMobRuntimeMode {
    AutonomousHost,
    TurnDriven,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledMobBackendKind {
    Session,
    External,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ForkContextSpec {
    #[default]
    FullHistory,
    LastMessages {
        count: u32,
    },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceFailureClass {
    TargetMaterializationFailed,
    TargetMissing,
    TargetBusy,
    RuntimeRejected,
    MobRejected,
    LeaseLost,
    TransportError,
    InternalError,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReceiptStage {
    Planned,
    Claimed,
    DispatchStarted,
    DispatchAccepted,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    DeliveryFailed,
    LeaseExpired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub receipt_id: Uuid,
    pub occurrence_id: OccurrenceId,
    pub attempt: u32,
    pub stage: DeliveryReceiptStage,
    pub recorded_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<OccurrenceFailureClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialized_session_id: Option<SessionId>,
}

impl DeliveryReceipt {
    pub fn new(occurrence_id: OccurrenceId, attempt: u32, stage: DeliveryReceiptStage) -> Self {
        Self {
            receipt_id: Uuid::now_v7(),
            occurrence_id,
            attempt,
            stage,
            recorded_at_utc: Utc::now(),
            correlation_id: None,
            detail: None,
            failure_class: None,
            materialized_session_id: None,
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    pub schedule_id: ScheduleId,
    pub phase: SchedulePhase,
    pub revision: ScheduleRevision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub trigger: TriggerSpec,
    pub target: TargetBinding,
    #[serde(default)]
    pub misfire_policy: MisfirePolicy,
    #[serde(default)]
    pub overlap_policy: OverlapPolicy,
    #[serde(default)]
    pub missing_target_policy: MissingTargetPolicy,
    #[serde(default = "default_planning_horizon_days")]
    pub planning_horizon_days: u32,
    #[serde(default = "default_planning_horizon_occurrences")]
    pub planning_horizon_occurrences: u32,
    pub next_occurrence_ordinal: OccurrenceOrdinal,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_cursor_utc: Option<DateTime<Utc>>,
    pub created_at_utc: DateTime<Utc>,
    pub updated_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

impl Schedule {
    pub fn new(request: CreateScheduleRequest) -> Self {
        let now = Utc::now();
        Self {
            schedule_id: ScheduleId::new(),
            phase: SchedulePhase::Active,
            revision: ScheduleRevision::initial(),
            name: request.name,
            description: request.description,
            trigger: request.trigger,
            target: request.target,
            misfire_policy: request.misfire_policy,
            overlap_policy: request.overlap_policy,
            missing_target_policy: request.missing_target_policy,
            planning_horizon_days: request
                .planning_horizon_days
                .unwrap_or(DEFAULT_PLANNING_HORIZON_DAYS),
            planning_horizon_occurrences: request
                .planning_horizon_occurrences
                .unwrap_or(DEFAULT_PLANNING_HORIZON_OCCURRENCES),
            next_occurrence_ordinal: OccurrenceOrdinal(0),
            planning_cursor_utc: None,
            created_at_utc: now,
            updated_at_utc: now,
            deleted_at_utc: None,
            labels: request.labels,
        }
    }

    pub fn bump_revision(&mut self) {
        self.revision = self.revision.next();
        self.updated_at_utc = Utc::now();
    }

    pub fn touch(&mut self) {
        self.updated_at_utc = Utc::now();
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Occurrence {
    pub occurrence_id: OccurrenceId,
    pub schedule_id: ScheduleId,
    pub schedule_revision: ScheduleRevision,
    pub occurrence_ordinal: OccurrenceOrdinal,
    pub phase: OccurrencePhase,
    pub due_at_utc: DateTime<Utc>,
    pub trigger_snapshot: TriggerSpec,
    pub target_snapshot: TargetBinding,
    pub misfire_policy: MisfirePolicy,
    pub overlap_policy: OverlapPolicy,
    pub missing_target_policy: MissingTargetPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_correlation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_receipt: Option<DeliveryReceipt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<OccurrenceFailureClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<String>,
    #[serde(default)]
    pub attempt_count: u32,
    pub created_at_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatched_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_utc: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by_revision: Option<ScheduleRevision>,
}

impl Occurrence {
    pub fn planned_from_schedule(
        schedule: &Schedule,
        occurrence_ordinal: OccurrenceOrdinal,
        due_at_utc: DateTime<Utc>,
    ) -> Self {
        Self {
            occurrence_id: OccurrenceId::new(),
            schedule_id: schedule.schedule_id.clone(),
            schedule_revision: schedule.revision,
            occurrence_ordinal,
            phase: OccurrencePhase::Pending,
            due_at_utc,
            trigger_snapshot: schedule.trigger.clone(),
            target_snapshot: schedule.target.clone(),
            misfire_policy: schedule.misfire_policy.clone(),
            overlap_policy: schedule.overlap_policy.clone(),
            missing_target_policy: schedule.missing_target_policy.clone(),
            claimed_by: None,
            lease_expires_at_utc: None,
            delivery_correlation_id: None,
            last_receipt: None,
            failure_class: None,
            failure_detail: None,
            attempt_count: 0,
            created_at_utc: Utc::now(),
            claimed_at_utc: None,
            dispatched_at_utc: None,
            completed_at_utc: None,
            superseded_by_revision: None,
        }
    }

    pub fn is_claimable_at(&self, now_utc: DateTime<Utc>) -> bool {
        self.phase == OccurrencePhase::Pending
            && self.due_at_utc <= now_utc
            && self
                .lease_expires_at_utc
                .as_ref()
                .is_none_or(|expires_at| *expires_at <= now_utc)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateScheduleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub trigger: TriggerSpec,
    pub target: TargetBinding,
    #[serde(default)]
    pub misfire_policy: MisfirePolicy,
    #[serde(default)]
    pub overlap_policy: OverlapPolicy,
    #[serde(default)]
    pub missing_target_policy: MissingTargetPolicy,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_occurrences: Option<u32>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateScheduleRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<ScheduleRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub misfire_policy: Option<MisfirePolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap_policy: Option<OverlapPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_target_policy: Option<MissingTargetPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planning_horizon_occurrences: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

pub fn default_planning_horizon_days() -> u32 {
    DEFAULT_PLANNING_HORIZON_DAYS
}

pub fn default_planning_horizon_occurrences() -> u32 {
    DEFAULT_PLANNING_HORIZON_OCCURRENCES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_binding_round_trips_without_duplicate_type_fields() {
        let binding = TargetBinding::Session(SessionTargetBinding::ExactSession {
            session_id: SessionId::new(),
            action: ScheduledSessionAction::Prompt {
                prompt: ContentInput::from("scheduled hello"),
                system_prompt: None,
                render_metadata: None,
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
        });

        let value = serde_json::to_value(&binding).expect("target binding should serialize");
        assert_eq!(value["target_kind"], "session");
        assert_eq!(value["type"], "exact_session");

        let round_trip: TargetBinding =
            serde_json::from_value(value).expect("target binding should deserialize");
        assert_eq!(round_trip, binding);
    }
}
