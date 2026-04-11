use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

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
