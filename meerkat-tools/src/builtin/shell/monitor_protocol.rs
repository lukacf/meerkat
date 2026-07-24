//! Pure, bounded stdout/stderr protocol for durable script monitors.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

const HARD_MAX_LINE_BYTES: usize = 1024 * 1024;
const HARD_MAX_NOTIFICATIONS_PER_WINDOW: usize = 1_000;
const HARD_MIN_NOTIFICATION_WINDOW_MS: u64 = 1_000;
const HARD_MAX_NOTIFICATION_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;
const HARD_MAX_RETAINED_DIAGNOSTIC_BYTES: usize = 4 * 1024 * 1024;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MonitorOutputProtocol {
    #[default]
    FramedJsonl,
    Lines,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MonitorProtocolLimits {
    pub max_line_bytes: usize,
    pub max_notifications_per_window: usize,
    pub notification_window_ms: u64,
    pub max_retained_diagnostic_bytes: usize,
}

impl Default for MonitorProtocolLimits {
    fn default() -> Self {
        Self {
            max_line_bytes: 64 * 1024,
            max_notifications_per_window: 60,
            notification_window_ms: 60_000,
            max_retained_diagnostic_bytes: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorAction {
    Notify {
        key: String,
        title: String,
        message: String,
    },
    Checkpoint {
        value: String,
    },
    Progress {
        cursor: u64,
        message: String,
    },
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorSuppressionReason {
    NotificationRateLimited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorLineOutcome {
    Action(MonitorAction),
    Diagnostic,
    Suppressed {
        reason: MonitorSuppressionReason,
        total_suppressed: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MonitorProtocolHealth {
    pub rate_limited: bool,
    pub suppressed_notifications: u64,
    pub diagnostic_bytes_dropped: u64,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum MonitorProtocolError {
    #[error("monitor line has {actual} bytes; limit is {limit}")]
    LineTooLong { actual: usize, limit: usize },
    #[error("malformed monitor frame: {0}")]
    MalformedFrame(String),
    #[error("invalid monitor protocol limits: {0}")]
    InvalidLimits(String),
}

#[derive(Debug)]
pub struct MonitorProtocolDecoder {
    protocol: MonitorOutputProtocol,
    limits: MonitorProtocolLimits,
    notification_times_ms: VecDeque<u64>,
    line_sequence: u64,
    retained_diagnostics: Vec<u8>,
    health: MonitorProtocolHealth,
    last_observed_at_ms: Option<u64>,
}

impl MonitorProtocolDecoder {
    pub fn new(
        protocol: MonitorOutputProtocol,
        limits: MonitorProtocolLimits,
    ) -> Result<Self, MonitorProtocolError> {
        if limits.max_line_bytes == 0 {
            return Err(MonitorProtocolError::InvalidLimits(
                "max_line_bytes must be positive".into(),
            ));
        }
        if limits.max_line_bytes > HARD_MAX_LINE_BYTES {
            return Err(MonitorProtocolError::InvalidLimits(format!(
                "max_line_bytes must not exceed {HARD_MAX_LINE_BYTES}"
            )));
        }
        if limits.max_notifications_per_window == 0 || limits.notification_window_ms == 0 {
            return Err(MonitorProtocolError::InvalidLimits(
                "notification rate window and budget must be positive".into(),
            ));
        }
        if limits.max_notifications_per_window > HARD_MAX_NOTIFICATIONS_PER_WINDOW {
            return Err(MonitorProtocolError::InvalidLimits(format!(
                "max_notifications_per_window must not exceed \
                 {HARD_MAX_NOTIFICATIONS_PER_WINDOW}"
            )));
        }
        if !(HARD_MIN_NOTIFICATION_WINDOW_MS..=HARD_MAX_NOTIFICATION_WINDOW_MS)
            .contains(&limits.notification_window_ms)
        {
            return Err(MonitorProtocolError::InvalidLimits(format!(
                "notification_window_ms must be between {HARD_MIN_NOTIFICATION_WINDOW_MS} and \
                 {HARD_MAX_NOTIFICATION_WINDOW_MS}"
            )));
        }
        if limits.max_retained_diagnostic_bytes == 0 {
            return Err(MonitorProtocolError::InvalidLimits(
                "max_retained_diagnostic_bytes must be positive".into(),
            ));
        }
        if limits.max_retained_diagnostic_bytes > HARD_MAX_RETAINED_DIAGNOSTIC_BYTES {
            return Err(MonitorProtocolError::InvalidLimits(format!(
                "max_retained_diagnostic_bytes must not exceed \
                 {HARD_MAX_RETAINED_DIAGNOSTIC_BYTES}"
            )));
        }
        Ok(Self {
            protocol,
            limits,
            notification_times_ms: VecDeque::new(),
            line_sequence: 0,
            retained_diagnostics: Vec::new(),
            health: MonitorProtocolHealth::default(),
            last_observed_at_ms: None,
        })
    }

    pub fn decode_stdout_line_at(
        &mut self,
        line: &str,
        observed_at_ms: u64,
    ) -> Result<MonitorLineOutcome, MonitorProtocolError> {
        self.validate_line(line)?;
        let action = match self.protocol {
            MonitorOutputProtocol::Lines => {
                self.line_sequence = self.line_sequence.checked_add(1).ok_or_else(|| {
                    MonitorProtocolError::MalformedFrame(
                        "line notification sequence exhausted u64".into(),
                    )
                })?;
                Some(MonitorAction::Notify {
                    key: format!("line:{}", self.line_sequence),
                    title: "Monitor notification".into(),
                    message: validate_frame_text("line notification", line)?,
                })
            }
            MonitorOutputProtocol::FramedJsonl => {
                let trimmed = line.trim();
                if !trimmed.starts_with('{') {
                    self.retain_diagnostic(line);
                    return Ok(MonitorLineOutcome::Diagnostic);
                }
                Some(parse_frame(trimmed)?)
            }
        };
        let Some(action) = action else {
            self.retain_diagnostic(line);
            return Ok(MonitorLineOutcome::Diagnostic);
        };
        if !matches!(action, MonitorAction::Notify { .. }) {
            return Ok(MonitorLineOutcome::Action(action));
        }
        if !self.admit_notification(observed_at_ms) {
            self.health.suppressed_notifications =
                self.health.suppressed_notifications.saturating_add(1);
            self.health.rate_limited = true;
            return Ok(MonitorLineOutcome::Suppressed {
                reason: MonitorSuppressionReason::NotificationRateLimited,
                total_suppressed: self.health.suppressed_notifications,
            });
        }
        self.health.rate_limited = false;
        Ok(MonitorLineOutcome::Action(action))
    }

    pub fn decode_stderr_line(&mut self, line: &str) -> Result<(), MonitorProtocolError> {
        self.validate_line(line)?;
        self.retain_diagnostic(line);
        Ok(())
    }

    pub fn retained_diagnostics(&self) -> String {
        String::from_utf8_lossy(&self.retained_diagnostics).into_owned()
    }

    pub const fn health(&self) -> MonitorProtocolHealth {
        self.health
    }

    fn validate_line(&self, line: &str) -> Result<(), MonitorProtocolError> {
        let actual = line.len();
        if actual > self.limits.max_line_bytes {
            return Err(MonitorProtocolError::LineTooLong {
                actual,
                limit: self.limits.max_line_bytes,
            });
        }
        Ok(())
    }

    fn admit_notification(&mut self, observed_at_ms: u64) -> bool {
        if self
            .last_observed_at_ms
            .is_some_and(|last| observed_at_ms < last)
        {
            self.notification_times_ms.clear();
        }
        self.last_observed_at_ms = Some(observed_at_ms);
        while self.notification_times_ms.front().is_some_and(|at| {
            observed_at_ms.saturating_sub(*at) >= self.limits.notification_window_ms
        }) {
            self.notification_times_ms.pop_front();
        }
        if self.notification_times_ms.len() >= self.limits.max_notifications_per_window {
            return false;
        }
        self.notification_times_ms.push_back(observed_at_ms);
        true
    }

    fn retain_diagnostic(&mut self, line: &str) {
        if !self.retained_diagnostics.is_empty() {
            self.retained_diagnostics.push(b'\n');
        }
        self.retained_diagnostics.extend_from_slice(line.as_bytes());
        let overflow = self
            .retained_diagnostics
            .len()
            .saturating_sub(self.limits.max_retained_diagnostic_bytes);
        if overflow > 0 {
            self.retained_diagnostics.drain(..overflow);
            self.health.diagnostic_bytes_dropped = self
                .health
                .diagnostic_bytes_dropped
                .saturating_add(u64::try_from(overflow).unwrap_or(u64::MAX));
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RawMonitorFrame {
    Notify {
        key: String,
        #[serde(default = "default_notification_title")]
        title: String,
        message: String,
    },
    Checkpoint {
        value: String,
    },
    Progress {
        cursor: u64,
        message: String,
    },
    Complete,
}

fn default_notification_title() -> String {
    "Monitor notification".into()
}

fn parse_frame(line: &str) -> Result<MonitorAction, MonitorProtocolError> {
    let raw: RawMonitorFrame = serde_json::from_str(line)
        .map_err(|error| MonitorProtocolError::MalformedFrame(error.to_string()))?;
    match raw {
        RawMonitorFrame::Notify {
            key,
            title,
            message,
        } => Ok(MonitorAction::Notify {
            key: validate_frame_text("notification key", &key)?,
            title: validate_frame_text("notification title", &title)?,
            message: validate_frame_text("notification message", &message)?,
        }),
        RawMonitorFrame::Checkpoint { value } => Ok(MonitorAction::Checkpoint {
            value: validate_frame_text("checkpoint", &value)?,
        }),
        RawMonitorFrame::Progress { cursor, message } => {
            if cursor == 0 {
                return Err(MonitorProtocolError::MalformedFrame(
                    "progress cursor must be positive".into(),
                ));
            }
            Ok(MonitorAction::Progress {
                cursor,
                message: validate_frame_text("progress message", &message)?,
            })
        }
        RawMonitorFrame::Complete => Ok(MonitorAction::Complete),
    }
}

fn validate_frame_text(label: &str, value: &str) -> Result<String, MonitorProtocolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return Err(MonitorProtocolError::MalformedFrame(format!(
            "{label} must be non-empty and contain no control characters"
        )));
    }
    Ok(trimmed.to_string())
}
