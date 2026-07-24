//! Pure predicate-watch contracts.
//!
//! Time-based execution is deliberately represented by an existing Schedule
//! identity. This module owns comparison/checkpoint semantics only; it has no
//! timer, task registry, network client, or lifecycle authority.

use serde::{Deserialize, Serialize};

use crate::{DetachedJobError, JobNotification, RestartClass};

const MIN_POLL_INTERVAL_SECS: u64 = 30;
const MAX_POLL_CONCURRENCY: u32 = 64;
const MAX_SOURCE_BUDGET_PER_MINUTE: u32 = 10_000;
const MAX_ID_BYTES: usize = 1_024;
const MAX_SOURCE_COMPONENT_BYTES: usize = 16 * 1_024;
const MAX_OBSERVATION_KEY_BYTES: usize = 4 * 1_024;
const MAX_OBSERVATION_MESSAGE_BYTES: usize = 64 * 1_024;

macro_rules! predicate_id {
    ($name:ident, $label:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, PredicateWatchError> {
                let value = value.into();
                let trimmed = value.trim();
                if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
                    return Err(PredicateWatchError::InvalidComponent {
                        label: $label,
                        detail: "must be non-empty and contain no control characters".into(),
                    });
                }
                if trimmed.len() > MAX_ID_BYTES {
                    return Err(PredicateWatchError::InvalidComponent {
                        label: $label,
                        detail: format!("must not exceed {MAX_ID_BYTES} bytes"),
                    });
                }
                Ok(Self(trimmed.to_string()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

predicate_id!(PredicateWatchId, "predicate watch id");
predicate_id!(ScheduleIdRef, "schedule id");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PredicateSource {
    StableHttp {
        url: String,
        conditional_requests: bool,
    },
    PersistentFile {
        path: String,
    },
    DurablePush {
        adapter: String,
        cursor_name: String,
    },
    HostDependent {
        adapter: String,
    },
}

impl PredicateSource {
    pub const fn restart_class(&self) -> RestartClass {
        match self {
            Self::StableHttp { .. } => RestartClass::Replayable,
            Self::PersistentFile { .. } => RestartClass::CheckpointResumable,
            Self::DurablePush { .. } => RestartClass::Adoptable,
            Self::HostDependent { .. } => RestartClass::NonResumable,
        }
    }

    fn validate(&self) -> Result<(), PredicateWatchError> {
        match self {
            Self::StableHttp { url, .. } => validate_component("stable HTTP URL", url),
            Self::PersistentFile { path } => validate_component("persistent file path", path),
            Self::DurablePush {
                adapter,
                cursor_name,
            } => {
                validate_component("durable push adapter", adapter)?;
                validate_component("durable push cursor", cursor_name)
            }
            Self::HostDependent { adapter } => {
                validate_component("host-dependent adapter", adapter)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PredicateComparison {
    Changed,
    Equals { expected: String },
    NumericAtLeast { threshold: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PredicatePollingPolicy {
    interval_secs: u64,
    max_concurrency: u32,
    jitter_percent: u8,
    source_budget_per_minute: u32,
    max_backoff_secs: u64,
}

impl PredicatePollingPolicy {
    pub fn new(
        interval_secs: u64,
        max_concurrency: u32,
        jitter_percent: u8,
        source_budget_per_minute: u32,
        max_backoff_secs: u64,
    ) -> Result<Self, PredicateWatchError> {
        if interval_secs < MIN_POLL_INTERVAL_SECS {
            return Err(PredicateWatchError::PollingIntervalTooShort {
                requested_secs: interval_secs,
                minimum_secs: MIN_POLL_INTERVAL_SECS,
            });
        }
        if max_concurrency == 0 || source_budget_per_minute == 0 {
            return Err(PredicateWatchError::InvalidPollingPolicy(
                "concurrency and per-source budget must be positive".into(),
            ));
        }
        if max_concurrency > MAX_POLL_CONCURRENCY {
            return Err(PredicateWatchError::InvalidPollingPolicy(format!(
                "concurrency must not exceed {MAX_POLL_CONCURRENCY}"
            )));
        }
        if source_budget_per_minute > MAX_SOURCE_BUDGET_PER_MINUTE {
            return Err(PredicateWatchError::InvalidPollingPolicy(format!(
                "per-source budget must not exceed {MAX_SOURCE_BUDGET_PER_MINUTE} per minute"
            )));
        }
        if jitter_percent > 100 {
            return Err(PredicateWatchError::InvalidPollingPolicy(
                "jitter percent must not exceed 100".into(),
            ));
        }
        if max_backoff_secs < interval_secs {
            return Err(PredicateWatchError::InvalidPollingPolicy(
                "maximum backoff must be at least the polling interval".into(),
            ));
        }
        Ok(Self {
            interval_secs,
            max_concurrency,
            jitter_percent,
            source_budget_per_minute,
            max_backoff_secs,
        })
    }

    pub const fn interval_secs(self) -> u64 {
        self.interval_secs
    }

    pub const fn max_concurrency(self) -> u32 {
        self.max_concurrency
    }

    pub const fn jitter_percent(self) -> u8 {
        self.jitter_percent
    }

    pub const fn source_budget_per_minute(self) -> u32 {
        self.source_budget_per_minute
    }

    pub const fn max_backoff_secs(self) -> u64 {
        self.max_backoff_secs
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PredicateWatch {
    watch_id: PredicateWatchId,
    schedule_id: ScheduleIdRef,
    source: PredicateSource,
    comparison: PredicateComparison,
    polling_policy: PredicatePollingPolicy,
}

impl PredicateWatch {
    pub fn scheduled(
        watch_id: PredicateWatchId,
        schedule_id: ScheduleIdRef,
        source: PredicateSource,
        comparison: PredicateComparison,
        polling_policy: PredicatePollingPolicy,
    ) -> Result<Self, PredicateWatchError> {
        source.validate()?;
        validate_comparison(&comparison)?;
        Ok(Self {
            watch_id,
            schedule_id,
            source,
            comparison,
            polling_policy,
        })
    }

    pub fn watch_id(&self) -> &PredicateWatchId {
        &self.watch_id
    }

    pub fn schedule_id(&self) -> &ScheduleIdRef {
        &self.schedule_id
    }

    pub fn source(&self) -> &PredicateSource {
        &self.source
    }

    pub const fn restart_class(&self) -> RestartClass {
        self.source.restart_class()
    }

    pub const fn polling_policy(&self) -> PredicatePollingPolicy {
        self.polling_policy
    }

    pub fn evaluate(
        &self,
        checkpoint: Option<&PredicateCheckpoint>,
        observation: PredicateObservation,
    ) -> Result<PredicateEvaluation, PredicateWatchError> {
        let PredicateObservation::Available {
            stable_key,
            message,
            numeric_value,
        } = observation
        else {
            let PredicateObservation::Unavailable { reason } = observation else {
                unreachable!();
            };
            return Ok(PredicateEvaluation::SourceUnavailable {
                checkpoint: checkpoint.cloned(),
                reason,
                retry_after_secs: self.polling_policy.interval_secs,
            });
        };
        let next = PredicateCheckpoint {
            stable_key: stable_key.clone(),
            message: message.clone(),
            numeric_value,
        };
        let Some(previous) = checkpoint else {
            return Ok(PredicateEvaluation::Baseline { checkpoint: next });
        };
        let crossed = match &self.comparison {
            PredicateComparison::Changed => previous.stable_key != stable_key,
            PredicateComparison::Equals { expected } => {
                previous.stable_key != stable_key && &stable_key == expected
            }
            PredicateComparison::NumericAtLeast { threshold } => {
                let previous_value = previous.numeric_value.unwrap_or(f64::NEG_INFINITY);
                let current = numeric_value.ok_or_else(|| {
                    PredicateWatchError::ObservationTypeMismatch(
                        "numeric comparison requires numeric observations".into(),
                    )
                })?;
                previous_value < *threshold && current >= *threshold
            }
        };
        if !crossed {
            return Ok(PredicateEvaluation::Unchanged { checkpoint: next });
        }
        let notification_id = format!(
            "notification_{}",
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!("{}:{stable_key}", self.watch_id.as_str()).as_bytes(),
            )
        );
        let notification = JobNotification::new(
            notification_id,
            format!("watch:{}:{stable_key}", self.watch_id.as_str()),
            "Predicate condition met",
            message,
        )
        .map_err(PredicateWatchError::Notification)?;
        Ok(PredicateEvaluation::Crossed {
            checkpoint: next,
            notification,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum PredicateObservation {
    Available {
        stable_key: String,
        message: String,
        numeric_value: Option<f64>,
    },
    Unavailable {
        reason: String,
    },
}

impl PredicateObservation {
    pub fn available(
        stable_key: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<Self, PredicateWatchError> {
        Ok(Self::Available {
            stable_key: validated_bounded_component(
                "observation stable key",
                stable_key.into(),
                MAX_OBSERVATION_KEY_BYTES,
            )?,
            message: validated_bounded_component(
                "observation message",
                message.into(),
                MAX_OBSERVATION_MESSAGE_BYTES,
            )?,
            numeric_value: None,
        })
    }

    pub fn numeric(
        stable_key: impl Into<String>,
        message: impl Into<String>,
        value: f64,
    ) -> Result<Self, PredicateWatchError> {
        if !value.is_finite() {
            return Err(PredicateWatchError::InvalidComponent {
                label: "numeric observation",
                detail: "must be finite".into(),
            });
        }
        Ok(Self::Available {
            stable_key: validated_bounded_component(
                "observation stable key",
                stable_key.into(),
                MAX_OBSERVATION_KEY_BYTES,
            )?,
            message: validated_bounded_component(
                "observation message",
                message.into(),
                MAX_OBSERVATION_MESSAGE_BYTES,
            )?,
            numeric_value: Some(value),
        })
    }

    pub fn unavailable(reason: impl Into<String>) -> Result<Self, PredicateWatchError> {
        Ok(Self::Unavailable {
            reason: validated_bounded_component(
                "source unavailability reason",
                reason.into(),
                MAX_OBSERVATION_KEY_BYTES,
            )?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PredicateCheckpoint {
    stable_key: String,
    message: String,
    numeric_value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PredicateEvaluation {
    Baseline {
        checkpoint: PredicateCheckpoint,
    },
    Unchanged {
        checkpoint: PredicateCheckpoint,
    },
    Crossed {
        checkpoint: PredicateCheckpoint,
        notification: JobNotification,
    },
    SourceUnavailable {
        checkpoint: Option<PredicateCheckpoint>,
        reason: String,
        retry_after_secs: u64,
    },
}

impl PredicateEvaluation {
    pub fn checkpoint(&self) -> Option<&PredicateCheckpoint> {
        match self {
            Self::Baseline { checkpoint }
            | Self::Unchanged { checkpoint }
            | Self::Crossed { checkpoint, .. } => Some(checkpoint),
            Self::SourceUnavailable { checkpoint, .. } => checkpoint.as_ref(),
        }
    }

    pub fn notification(&self) -> Option<&JobNotification> {
        match self {
            Self::Crossed { notification, .. } => Some(notification),
            Self::Baseline { .. } | Self::Unchanged { .. } | Self::SourceUnavailable { .. } => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PredicateWatchError {
    #[error("invalid {label}: {detail}")]
    InvalidComponent { label: &'static str, detail: String },
    #[error("polling interval {requested_secs}s is below the minimum {minimum_secs}s")]
    PollingIntervalTooShort {
        requested_secs: u64,
        minimum_secs: u64,
    },
    #[error("invalid polling policy: {0}")]
    InvalidPollingPolicy(String),
    #[error("predicate observation type mismatch: {0}")]
    ObservationTypeMismatch(String),
    #[error("cannot construct predicate notification: {0}")]
    Notification(DetachedJobError),
}

fn validate_comparison(comparison: &PredicateComparison) -> Result<(), PredicateWatchError> {
    match comparison {
        PredicateComparison::Changed => Ok(()),
        PredicateComparison::Equals { expected } => {
            validate_component("predicate expected value", expected)
        }
        PredicateComparison::NumericAtLeast { threshold } if threshold.is_finite() => Ok(()),
        PredicateComparison::NumericAtLeast { .. } => Err(PredicateWatchError::InvalidComponent {
            label: "numeric threshold",
            detail: "must be finite".into(),
        }),
    }
}

fn validate_component(label: &'static str, value: &str) -> Result<(), PredicateWatchError> {
    validated_bounded_component(label, value.to_string(), MAX_SOURCE_COMPONENT_BYTES).map(|_| ())
}

fn validated_component(label: &'static str, value: String) -> Result<String, PredicateWatchError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(PredicateWatchError::InvalidComponent {
            label,
            detail: "must be non-empty and contain no control characters".into(),
        });
    }
    Ok(trimmed.to_string())
}

fn validated_bounded_component(
    label: &'static str,
    value: String,
    max_bytes: usize,
) -> Result<String, PredicateWatchError> {
    let value = validated_component(label, value)?;
    if value.len() > max_bytes {
        return Err(PredicateWatchError::InvalidComponent {
            label,
            detail: format!("must not exceed {max_bytes} bytes"),
        });
    }
    Ok(value)
}
