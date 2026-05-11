use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub enum ScheduleToolName {
    #[serde(rename = "meerkat_schedule_create")]
    Create,
    #[serde(rename = "meerkat_schedule_get")]
    Get,
    #[serde(rename = "meerkat_schedule_list")]
    List,
    #[serde(rename = "meerkat_schedule_update")]
    Update,
    #[serde(rename = "meerkat_schedule_pause")]
    Pause,
    #[serde(rename = "meerkat_schedule_resume")]
    Resume,
    #[serde(rename = "meerkat_schedule_delete")]
    Delete,
    #[serde(rename = "meerkat_schedule_occurrences")]
    Occurrences,
}

impl ScheduleToolName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "meerkat_schedule_create",
            Self::Get => "meerkat_schedule_get",
            Self::List => "meerkat_schedule_list",
            Self::Update => "meerkat_schedule_update",
            Self::Pause => "meerkat_schedule_pause",
            Self::Resume => "meerkat_schedule_resume",
            Self::Delete => "meerkat_schedule_delete",
            Self::Occurrences => "meerkat_schedule_occurrences",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EmptyScheduleToolArguments {}

impl EmptyScheduleToolArguments {
    fn is_empty(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScheduleToolIdArguments {
    pub schedule_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ScheduleToolUpdateArguments {
    pub schedule_id: String,
    #[serde(flatten)]
    pub update: meerkat_schedule::UpdateScheduleRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "name")]
pub enum ScheduleToolCallParams {
    #[serde(rename = "meerkat_schedule_create")]
    Create {
        arguments: meerkat_schedule::CreateScheduleRequest,
    },
    #[serde(rename = "meerkat_schedule_get")]
    Get { arguments: ScheduleToolIdArguments },
    #[serde(rename = "meerkat_schedule_list")]
    List {
        #[serde(default, skip_serializing_if = "EmptyScheduleToolArguments::is_empty")]
        arguments: EmptyScheduleToolArguments,
    },
    #[serde(rename = "meerkat_schedule_update")]
    Update {
        arguments: ScheduleToolUpdateArguments,
    },
    #[serde(rename = "meerkat_schedule_pause")]
    Pause { arguments: ScheduleToolIdArguments },
    #[serde(rename = "meerkat_schedule_resume")]
    Resume { arguments: ScheduleToolIdArguments },
    #[serde(rename = "meerkat_schedule_delete")]
    Delete { arguments: ScheduleToolIdArguments },
    #[serde(rename = "meerkat_schedule_occurrences")]
    Occurrences { arguments: ScheduleToolIdArguments },
}

impl ScheduleToolCallParams {
    pub const fn name(&self) -> ScheduleToolName {
        match self {
            Self::Create { .. } => ScheduleToolName::Create,
            Self::Get { .. } => ScheduleToolName::Get,
            Self::List { .. } => ScheduleToolName::List,
            Self::Update { .. } => ScheduleToolName::Update,
            Self::Pause { .. } => ScheduleToolName::Pause,
            Self::Resume { .. } => ScheduleToolName::Resume,
            Self::Delete { .. } => ScheduleToolName::Delete,
            Self::Occurrences { .. } => ScheduleToolName::Occurrences,
        }
    }

    pub fn arguments_json(&self) -> Result<Value, serde_json::Error> {
        match self {
            Self::Create { arguments } => serde_json::to_value(arguments),
            Self::Get { arguments }
            | Self::Pause { arguments }
            | Self::Resume { arguments }
            | Self::Delete { arguments }
            | Self::Occurrences { arguments } => serde_json::to_value(arguments),
            Self::List { arguments } => serde_json::to_value(arguments),
            Self::Update { arguments } => serde_json::to_value(arguments),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleToolDescriptor {
    pub name: ScheduleToolName,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleToolsResult {
    pub tools: Vec<ScheduleToolDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[allow(clippy::large_enum_variant)]
#[serde(untagged)]
pub enum ScheduleToolCallResult {
    Schedule(meerkat_schedule::Schedule),
    List(ScheduleListResult),
    Occurrences(ScheduleOccurrencesResult),
}

impl ScheduleToolCallResult {
    pub fn from_tool_value(
        name: ScheduleToolName,
        value: Value,
    ) -> Result<Self, serde_json::Error> {
        match name {
            ScheduleToolName::Create
            | ScheduleToolName::Get
            | ScheduleToolName::Update
            | ScheduleToolName::Pause
            | ScheduleToolName::Resume
            | ScheduleToolName::Delete => serde_json::from_value(value).map(Self::Schedule),
            ScheduleToolName::List => serde_json::from_value(value).map(Self::List),
            ScheduleToolName::Occurrences => serde_json::from_value(value).map(Self::Occurrences),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ListSchedulesParams {
    pub labels: Option<BTreeMap<String, String>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleIdParams {
    pub schedule_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleOccurrencesParams {
    pub schedule_id: String,
    pub include_terminal: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct UpdateScheduleParams {
    pub schedule_id: String,
    #[serde(flatten)]
    pub update: meerkat_schedule::UpdateScheduleRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleListResult {
    pub schedules: Vec<meerkat_schedule::Schedule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleOccurrencesResult {
    pub occurrences: Vec<meerkat_schedule::Occurrence>,
}
