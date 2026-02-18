use crate::model::{MobEventCategory, MobEventKind, NewMobEvent};
use chrono::{DateTime, Utc};
use serde_json::Value;

pub(crate) struct MobEventBuilder {
    timestamp: DateTime<Utc>,
    category: MobEventCategory,
    mob_id: String,
    run_id: Option<String>,
    flow_id: Option<String>,
    step_id: Option<String>,
    meerkat_id: Option<String>,
    kind: MobEventKind,
    payload: Value,
}

impl MobEventBuilder {
    pub(crate) fn new(mob_id: String, category: MobEventCategory, kind: MobEventKind) -> Self {
        Self {
            timestamp: Utc::now(),
            category,
            mob_id,
            run_id: None,
            flow_id: None,
            step_id: None,
            meerkat_id: None,
            kind,
            payload: Value::Null,
        }
    }

    pub(crate) fn run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    pub(crate) fn flow_id(mut self, flow_id: impl Into<String>) -> Self {
        self.flow_id = Some(flow_id.into());
        self
    }

    pub(crate) fn step_id(mut self, step_id: impl Into<String>) -> Self {
        self.step_id = Some(step_id.into());
        self
    }

    pub(crate) fn meerkat_id(mut self, meerkat_id: impl Into<String>) -> Self {
        self.meerkat_id = Some(meerkat_id.into());
        self
    }

    pub(crate) fn payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub(crate) fn build(self) -> NewMobEvent {
        NewMobEvent {
            timestamp: self.timestamp,
            category: self.category,
            mob_id: self.mob_id,
            run_id: self.run_id,
            flow_id: self.flow_id,
            step_id: self.step_id,
            meerkat_id: self.meerkat_id,
            kind: self.kind,
            payload: self.payload,
        }
    }
}
