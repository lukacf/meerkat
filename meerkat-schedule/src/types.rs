use chrono::{DateTime, Duration, Utc};
use meerkat_core::ops::ToolAccessPolicy;
use meerkat_core::types::RenderMetadata;
use meerkat_core::{ContentInput, OutputSchema, PeerMeta, Provider, SessionId};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

const DEFAULT_PLANNING_HORIZON_DAYS: u32 = 30;
const DEFAULT_PLANNING_HORIZON_OCCURRENCES: u32 = 64;
const DEFAULT_SKIP_MISFIRE_GRACE_SECONDS: i64 = 30;

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

    pub fn holds_active_lease(self) -> bool {
        matches!(
            self,
            Self::Claimed | Self::Dispatching | Self::AwaitingCompletion
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum CalendarFieldSpec {
    #[default]
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

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MisfirePolicy {
    /// Skip materially late pending occurrences instead of catching them up.
    ///
    /// The scheduler applies a small internal grace window so normal tick
    /// jitter does not immediately misfire on-time work.
    #[default]
    Skip,
    /// Catch up overdue pending occurrences only while they remain within the
    /// configured lateness window.
    CatchUpWithin { window_seconds: u64 },
}

impl MisfirePolicy {
    fn catch_up_window(&self) -> Option<Duration> {
        match self {
            Self::Skip => Some(Duration::seconds(DEFAULT_SKIP_MISFIRE_GRACE_SECONDS)),
            Self::CatchUpWithin { window_seconds } => {
                let seconds = i64::try_from(*window_seconds).unwrap_or(i64::MAX);
                Some(Duration::seconds(seconds))
            }
        }
    }

    fn allows_pending_delivery_at(
        &self,
        due_at_utc: DateTime<Utc>,
        now_utc: DateTime<Utc>,
    ) -> bool {
        if now_utc <= due_at_utc {
            return true;
        }
        self.catch_up_window()
            .and_then(|window| due_at_utc.checked_add_signed(window))
            .is_some_and(|deadline| now_utc <= deadline)
    }

    fn misfire_detail(&self, due_at_utc: DateTime<Utc>, now_utc: DateTime<Utc>) -> String {
        let lateness_seconds = now_utc
            .signed_duration_since(due_at_utc)
            .num_seconds()
            .max(0);
        match self {
            Self::Skip => format!(
                "missed scheduled time by {lateness_seconds}s; skip policy does not catch up materially late pending occurrences"
            ),
            Self::CatchUpWithin { window_seconds } => format!(
                "missed scheduled time by {lateness_seconds}s, exceeding catch-up window of {window_seconds}s"
            ),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    AllowConcurrent,
    #[default]
    SkipIfRunning,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingTargetPolicy {
    Skip,
    #[default]
    MarkMisfired,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "target_kind", rename_all = "snake_case")]
pub enum TargetBinding {
    Session(Box<SessionTargetBinding>),
    Mob(Box<MobTargetBinding>),
}

impl TargetBinding {
    pub fn session(binding: SessionTargetBinding) -> Self {
        Self::Session(Box::new(binding))
    }

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
        create: Box<SessionMaterializationSpec>,
        action: ScheduledSessionAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bound_session_id: Option<SessionId>,
    },
}

impl SessionTargetBinding {
    pub fn materialize_on_demand(
        create: SessionMaterializationSpec,
        action: ScheduledSessionAction,
    ) -> Self {
        Self::MaterializeOnDemandSession {
            create: Box::new(create),
            action,
            bound_session_id: None,
        }
    }

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMaterializationSpec {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "output_schema_json"
    )]
    pub output_schema: Option<OutputSchema>,
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
        self.output_schema = Some(output_schema);
        self
    }
}

impl PartialEq for SessionMaterializationSpec {
    fn eq(&self, other: &Self) -> bool {
        self.model == other.model
            && self.system_prompt == other.system_prompt
            && self.max_tokens == other.max_tokens
            && self.provider == other.provider
            && self.output_schema == other.output_schema
            && self.structured_output_retries == other.structured_output_retries
            && self.provider_params == other.provider_params
            && self.comms_name == other.comms_name
            && self.peer_meta == other.peer_meta
            && self.labels == other.labels
            && self.preload_skills == other.preload_skills
            && self.additional_instructions == other.additional_instructions
            && self.realm_id == other.realm_id
            && self.instance_id == other.instance_id
            && self.backend == other.backend
            && self.config_generation == other.config_generation
            && self.keep_alive == other.keep_alive
            && self.app_context == other.app_context
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        #[cfg_attr(feature = "schema", schemars(with = "serde_json::Value"))]
        params: Box<RawValue>,
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

impl PartialEq for MobTargetBinding {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Member {
                    mob_id: left_mob_id,
                    member_id: left_member_id,
                    action: left_action,
                },
                Self::Member {
                    mob_id: right_mob_id,
                    member_id: right_member_id,
                    action: right_action,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_member_id == right_member_id
                    && left_action == right_action
            }
            (
                Self::Flow {
                    mob_id: left_mob_id,
                    flow_id: left_flow_id,
                    params: left_params,
                },
                Self::Flow {
                    mob_id: right_mob_id,
                    flow_id: right_flow_id,
                    params: right_params,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_flow_id == right_flow_id
                    && left_params.get() == right_params.get()
            }
            (
                Self::SpawnHelper {
                    mob_id: left_mob_id,
                    member_id: left_member_id,
                    prompt: left_prompt,
                    options: left_options,
                },
                Self::SpawnHelper {
                    mob_id: right_mob_id,
                    member_id: right_member_id,
                    prompt: right_prompt,
                    options: right_options,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_member_id == right_member_id
                    && left_prompt == right_prompt
                    && left_options == right_options
            }
            (
                Self::ForkHelper {
                    mob_id: left_mob_id,
                    source_member_id: left_source_member_id,
                    member_id: left_member_id,
                    prompt: left_prompt,
                    fork_context: left_fork_context,
                    options: left_options,
                },
                Self::ForkHelper {
                    mob_id: right_mob_id,
                    source_member_id: right_source_member_id,
                    member_id: right_member_id,
                    prompt: right_prompt,
                    fork_context: right_fork_context,
                    options: right_options,
                },
            ) => {
                left_mob_id == right_mob_id
                    && left_source_member_id == right_source_member_id
                    && left_member_id == right_member_id
                    && left_prompt == right_prompt
                    && left_fork_context == right_fork_context
                    && left_options == right_options
            }
            _ => false,
        }
    }
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

/// How the scheduled helper's tool surface should be determined.
///
/// Schedule-local mirror of `meerkat_mob::SpawnTooling`, wire-compatible.
/// `InheritParent` and `Minimal` require an active parent agent context and
/// are rejected by public schedule APIs — they are only valid when a schedule
/// is created through agent tools (where the parent can snapshot its tools).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ScheduleSpawnTooling {
    /// Inherit the parent's currently visible tools (ToolScope snapshot).
    InheritParent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_overlay: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deny_overlay: Option<Vec<String>>,
    },
    /// Minimal: only comms tools.
    Minimal,
    /// Use a specific named profile for model/tool resolution.
    Profile {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        allow_overlay: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        deny_overlay: Option<Vec<String>>,
    },
}

impl ScheduleSpawnTooling {
    /// Returns true if this tooling mode requires an active parent agent context.
    pub fn requires_parent_context(&self) -> bool {
        matches!(self, Self::InheritParent { .. } | Self::Minimal)
    }
}

/// Pre-resolved spawn snapshot captured at schedule creation time.
///
/// When `ScheduleSpawnTooling::InheritParent` or `Minimal` is specified through
/// agent tools, the parent's visible tool set is snapshotted and stored here.
/// At execution time, the schedule driver uses this snapshot directly — no
/// parent context is needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSpawnSnapshot {
    /// The tool filter to apply as the child's inherited base filter.
    pub tool_filter: meerkat_core::tool_scope::ToolFilter,
    /// The model to use for the child agent.
    pub model: String,
    /// Optional provider-specific parameters for the child agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<serde_json::Value>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HelperOptionsSpec {
    /// Role name (profile key) for the helper in the mob roster.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "profile_name"
    )]
    pub role_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<ScheduledMobRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<ScheduledMobBackendKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_access_policy: Option<ToolAccessPolicy>,
    /// Tooling mode request for the helper's tool surface.
    ///
    /// This is an input-side field consumed during schedule creation.
    /// `InheritParent` and `Minimal` are resolved into `resolved_spawn_snapshot`
    /// by the agent tool path; they are rejected by public schedule APIs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub tooling: Option<ScheduleSpawnTooling>,
    /// Pre-resolved tool/model snapshot, populated when `tooling` is consumed.
    ///
    /// At execution time, the schedule driver reads this field directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub resolved_spawn_snapshot: Option<ResolvedSpawnSnapshot>,
}

impl HelperOptionsSpec {
    /// Validate that the tooling mode is acceptable for public schedule APIs.
    ///
    /// `InheritParent` and `Minimal` require an active parent agent context and
    /// are only valid when created through agent tools. Public APIs must reject them.
    pub fn validate_public_api(&self) -> Result<(), String> {
        if let Some(tooling) = &self.tooling
            && tooling.requires_parent_context()
        {
            return Err("schedule spawn tooling mode requires parent agent context \
                 (inherit_parent/minimal are only valid through agent tools)"
                .to_string());
        }
        Ok(())
    }
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
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
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
    #[cfg_attr(feature = "schema", schemars(skip))]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) claim_token: Option<Uuid>,
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
            claim_token: None,
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
        self.due_at_utc <= now_utc
            && (self.phase == OccurrencePhase::Pending || self.is_reclaimable_at(now_utc))
    }

    pub fn should_misfire_at(&self, now_utc: DateTime<Utc>) -> bool {
        self.phase == OccurrencePhase::Pending
            && self.due_at_utc <= now_utc
            && !self
                .misfire_policy
                .allows_pending_delivery_at(self.due_at_utc, now_utc)
    }

    pub fn misfire_detail_at(&self, now_utc: DateTime<Utc>) -> Option<String> {
        self.should_misfire_at(now_utc)
            .then(|| self.misfire_policy.misfire_detail(self.due_at_utc, now_utc))
    }

    pub fn is_reclaimable_at(&self, now_utc: DateTime<Utc>) -> bool {
        self.phase.holds_active_lease()
            && self
                .lease_expires_at_utc
                .is_some_and(|expires_at| expires_at <= now_utc)
    }

    pub fn claim_token(&self) -> Option<Uuid> {
        self.claim_token
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

#[cfg(all(test, feature = "schema"))]
#[allow(clippy::unwrap_used)]
mod schema_tests {
    use super::HelperOptionsSpec;

    #[test]
    fn helper_options_schema_exposes_typed_tool_access_policy_variants() {
        let schema_json = serde_json::to_string(&schemars::schema_for!(HelperOptionsSpec)).unwrap();

        assert!(
            schema_json.contains("allow_list"),
            "helper options schema should expose ToolAccessPolicy variants"
        );
        assert!(
            schema_json.contains("deny_list"),
            "helper options schema should expose ToolAccessPolicy variants"
        );
        assert!(
            schema_json.contains("inherit"),
            "helper options schema should expose ToolAccessPolicy variants"
        );
    }
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // -----------------------------------------------------------------------
    // ScheduleSpawnTooling serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn schedule_spawn_tooling_inherit_parent_roundtrip() {
        let tooling = ScheduleSpawnTooling::InheritParent {
            allow_overlay: Some(vec!["shell".into()]),
            deny_overlay: None,
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: ScheduleSpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn schedule_spawn_tooling_minimal_roundtrip() {
        let tooling = ScheduleSpawnTooling::Minimal;
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: ScheduleSpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn schedule_spawn_tooling_profile_roundtrip() {
        let tooling = ScheduleSpawnTooling::Profile {
            name: "worker-v2".into(),
            allow_overlay: None,
            deny_overlay: Some(vec!["dangerous".into()]),
        };
        let json = serde_json::to_string(&tooling).unwrap();
        let parsed: ScheduleSpawnTooling = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tooling);
    }

    #[test]
    fn schedule_spawn_tooling_requires_parent_context() {
        assert!(
            ScheduleSpawnTooling::InheritParent {
                allow_overlay: None,
                deny_overlay: None,
            }
            .requires_parent_context()
        );
        assert!(ScheduleSpawnTooling::Minimal.requires_parent_context());
        assert!(
            !ScheduleSpawnTooling::Profile {
                name: "x".into(),
                allow_overlay: None,
                deny_overlay: None,
            }
            .requires_parent_context()
        );
    }

    // -----------------------------------------------------------------------
    // ResolvedSpawnSnapshot serde roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn resolved_spawn_snapshot_roundtrip_allow_filter() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::Allow(HashSet::from([
                "shell".to_string(),
                "read_file".to_string(),
            ])),
            model: "claude-opus-4-6".into(),
            provider_params: Some(serde_json::json!({"thinking_budget": 8192})),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ResolvedSpawnSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn resolved_spawn_snapshot_roundtrip_deny_filter() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::Deny(HashSet::from([
                "dangerous_tool".to_string(),
            ])),
            model: "gpt-5.4".into(),
            provider_params: None,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ResolvedSpawnSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }

    #[test]
    fn resolved_spawn_snapshot_roundtrip_all_filter() {
        let snapshot = ResolvedSpawnSnapshot {
            tool_filter: meerkat_core::tool_scope::ToolFilter::All,
            model: "gemini-3.1-pro-preview".into(),
            provider_params: None,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let parsed: ResolvedSpawnSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, snapshot);
    }

    // -----------------------------------------------------------------------
    // HelperOptionsSpec with tooling/snapshot fields
    // -----------------------------------------------------------------------

    #[test]
    fn helper_options_spec_with_tooling_roundtrip() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Profile {
                name: "worker".into(),
                allow_overlay: None,
                deny_overlay: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: HelperOptionsSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn helper_options_spec_with_resolved_snapshot_roundtrip() {
        let spec = HelperOptionsSpec {
            resolved_spawn_snapshot: Some(ResolvedSpawnSnapshot {
                tool_filter: meerkat_core::tool_scope::ToolFilter::Allow(HashSet::from([
                    "shell".to_string()
                ])),
                model: "claude-sonnet-4-6".into(),
                provider_params: None,
            }),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: HelperOptionsSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, spec);
    }

    #[test]
    fn helper_options_spec_default_omits_tooling_fields() {
        let spec = HelperOptionsSpec::default();
        let json = serde_json::to_string(&spec).unwrap();
        assert!(!json.contains("tooling"));
        assert!(!json.contains("resolved_spawn_snapshot"));
    }

    // -----------------------------------------------------------------------
    // Public API validation
    // -----------------------------------------------------------------------

    #[test]
    fn validate_public_api_rejects_inherit_parent() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::InheritParent {
                allow_overlay: None,
                deny_overlay: None,
            }),
            ..Default::default()
        };
        let result = spec.validate_public_api();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("requires parent agent context")
        );
    }

    #[test]
    fn validate_public_api_rejects_minimal() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Minimal),
            ..Default::default()
        };
        assert!(spec.validate_public_api().is_err());
    }

    #[test]
    fn validate_public_api_accepts_profile() {
        let spec = HelperOptionsSpec {
            tooling: Some(ScheduleSpawnTooling::Profile {
                name: "worker".into(),
                allow_overlay: None,
                deny_overlay: None,
            }),
            ..Default::default()
        };
        assert!(spec.validate_public_api().is_ok());
    }

    #[test]
    fn validate_public_api_accepts_none_tooling() {
        let spec = HelperOptionsSpec::default();
        assert!(spec.validate_public_api().is_ok());
    }

    // -----------------------------------------------------------------------
    // role_name backward compat
    // -----------------------------------------------------------------------

    #[test]
    fn helper_options_spec_role_name_is_canonical_field() {
        let spec = HelperOptionsSpec {
            role_name: Some("worker".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("role_name"));
        assert!(!json.contains("profile_name"));
        let parsed: HelperOptionsSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role_name, Some("worker".into()));
    }

    #[test]
    fn helper_options_spec_profile_name_alias_deserializes() {
        let json = r#"{"profile_name":"legacy-worker"}"#;
        let parsed: HelperOptionsSpec = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.role_name, Some("legacy-worker".into()));
    }

    // -----------------------------------------------------------------------
    // Existing tests
    // -----------------------------------------------------------------------

    #[test]
    fn target_binding_round_trips_without_duplicate_type_fields() -> Result<(), String> {
        let binding = TargetBinding::session(SessionTargetBinding::ExactSession {
            session_id: SessionId::new(),
            action: ScheduledSessionAction::Prompt {
                prompt: ContentInput::from("scheduled hello"),
                system_prompt: None,
                render_metadata: None,
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
        });

        let value = serde_json::to_value(&binding).map_err(|error| error.to_string())?;
        assert_eq!(value["target_kind"], "session");
        assert_eq!(value["type"], "exact_session");

        let round_trip: TargetBinding =
            serde_json::from_value(value).map_err(|error| error.to_string())?;
        assert_eq!(round_trip, binding);
        Ok(())
    }
}
