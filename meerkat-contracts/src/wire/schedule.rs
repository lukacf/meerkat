use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ListSchedulesParams {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ScheduleIdParams {
    pub schedule_id: String,
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
