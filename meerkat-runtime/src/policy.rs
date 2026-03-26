//! §12 PolicyDecision — the output of the policy table.
//!
//! The runtime's policy table resolves each Input to a PolicyDecision
//! that determines how and when the input is applied, whether it wakes
//! the runtime, how it's queued, and when it's consumed.

use serde::{Deserialize, Serialize};

use crate::identifiers::PolicyVersion;

/// How the input should be applied to the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ApplyMode {
    /// Stage for application at the start of the next run.
    StageRunStart,
    /// Stage for application at any run boundary (start or checkpoint).
    StageRunBoundary,
    /// Inject immediately (no run boundary required).
    InjectNow,
    /// Do not apply (input is informational only).
    Ignore,
}

/// Whether the input should wake an idle runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WakeMode {
    /// Wake the runtime if idle.
    WakeIfIdle,
    /// Interrupt cooperative yielding points (e.g., wait tool) but don't
    /// cancel active work or wake an idle runtime.
    InterruptYielding,
    /// Do not wake (input will be processed at next natural run).
    None,
}

/// Queue ordering discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QueueMode {
    /// No queueing (immediate consumption).
    None,
    /// First-in, first-out ordering.
    Fifo,
    /// Coalesce with other inputs of the same type.
    Coalesce,
    /// Supersede earlier inputs with the same supersession key.
    Supersede,
    /// Priority ordering (higher priority first).
    Priority,
}

/// When the input is considered consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConsumePoint {
    /// Consumed when the input is accepted.
    OnAccept,
    /// Consumed when the input is applied (boundary executed).
    OnApply,
    /// Consumed when the run starts.
    OnRunStart,
    /// Consumed when the run completes.
    OnRunComplete,
    /// Consumed only on explicit acknowledgment.
    ExplicitAck,
}

/// Whether admitted work may interrupt a yielding runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum InterruptPolicy {
    #[default]
    None,
    InterruptYielding,
}

/// How the runtime should drain admitted work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DrainPolicy {
    #[default]
    QueueNextTurn,
    SteerBatch,
    Immediate,
    Ignore,
}

/// Where admitted work routes after policy resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RoutingDisposition {
    #[default]
    Queue,
    Steer,
    Immediate,
    Drop,
}

/// Full policy decision for an input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// How to apply the input.
    pub apply_mode: ApplyMode,
    /// Whether to wake the runtime.
    pub wake_mode: WakeMode,
    /// Queue ordering.
    pub queue_mode: QueueMode,
    /// When the input is consumed.
    pub consume_point: ConsumePoint,
    /// Whether yielding work may be interrupted.
    #[serde(default)]
    pub interrupt_policy: InterruptPolicy,
    /// How runtime drain ownership should handle this work.
    #[serde(default)]
    pub drain_policy: DrainPolicy,
    /// Where the work routes after admission.
    #[serde(default)]
    pub routing_disposition: RoutingDisposition,
    /// Whether to record this input in the conversation transcript.
    #[serde(default = "default_true")]
    pub record_transcript: bool,
    /// Whether to emit operator-visible content for this input.
    #[serde(default = "default_true")]
    pub emit_operator_content: bool,
    /// Policy version that produced this decision.
    pub policy_version: PolicyVersion,
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn apply_mode_serde() {
        for mode in [
            ApplyMode::StageRunStart,
            ApplyMode::StageRunBoundary,
            ApplyMode::InjectNow,
            ApplyMode::Ignore,
        ] {
            let json = serde_json::to_value(mode).unwrap();
            let parsed: ApplyMode = serde_json::from_value(json).unwrap();
            assert_eq!(mode, parsed);
        }
    }

    #[test]
    fn wake_mode_serde() {
        for mode in [
            WakeMode::WakeIfIdle,
            WakeMode::InterruptYielding,
            WakeMode::None,
        ] {
            let json = serde_json::to_value(mode).unwrap();
            let parsed: WakeMode = serde_json::from_value(json).unwrap();
            assert_eq!(mode, parsed);
        }
    }

    #[test]
    fn queue_mode_serde() {
        for mode in [
            QueueMode::None,
            QueueMode::Fifo,
            QueueMode::Coalesce,
            QueueMode::Supersede,
            QueueMode::Priority,
        ] {
            let json = serde_json::to_value(mode).unwrap();
            let parsed: QueueMode = serde_json::from_value(json).unwrap();
            assert_eq!(mode, parsed);
        }
    }

    #[test]
    fn consume_point_serde() {
        for point in [
            ConsumePoint::OnAccept,
            ConsumePoint::OnApply,
            ConsumePoint::OnRunStart,
            ConsumePoint::OnRunComplete,
            ConsumePoint::ExplicitAck,
        ] {
            let json = serde_json::to_value(point).unwrap();
            let parsed: ConsumePoint = serde_json::from_value(json).unwrap();
            assert_eq!(point, parsed);
        }
    }

    #[test]
    fn interrupt_policy_serde() {
        for policy in [InterruptPolicy::None, InterruptPolicy::InterruptYielding] {
            let json = serde_json::to_value(policy).unwrap();
            let parsed: InterruptPolicy = serde_json::from_value(json).unwrap();
            assert_eq!(policy, parsed);
        }
    }

    #[test]
    fn drain_policy_serde() {
        for policy in [
            DrainPolicy::QueueNextTurn,
            DrainPolicy::SteerBatch,
            DrainPolicy::Immediate,
            DrainPolicy::Ignore,
        ] {
            let json = serde_json::to_value(policy).unwrap();
            let parsed: DrainPolicy = serde_json::from_value(json).unwrap();
            assert_eq!(policy, parsed);
        }
    }

    #[test]
    fn routing_disposition_serde() {
        for disposition in [
            RoutingDisposition::Queue,
            RoutingDisposition::Steer,
            RoutingDisposition::Immediate,
            RoutingDisposition::Drop,
        ] {
            let json = serde_json::to_value(disposition).unwrap();
            let parsed: RoutingDisposition = serde_json::from_value(json).unwrap();
            assert_eq!(disposition, parsed);
        }
    }

    #[test]
    fn policy_decision_serde_roundtrip() {
        let decision = PolicyDecision {
            apply_mode: ApplyMode::StageRunStart,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            interrupt_policy: InterruptPolicy::None,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: PolicyVersion(1),
        };
        let json = serde_json::to_value(&decision).unwrap();
        let parsed: PolicyDecision = serde_json::from_value(json).unwrap();
        assert_eq!(decision, parsed);
    }

    #[test]
    fn policy_decision_ignore_on_accept() {
        let decision = PolicyDecision {
            apply_mode: ApplyMode::Ignore,
            wake_mode: WakeMode::None,
            queue_mode: QueueMode::None,
            consume_point: ConsumePoint::OnAccept,
            interrupt_policy: InterruptPolicy::None,
            drain_policy: DrainPolicy::Ignore,
            routing_disposition: RoutingDisposition::Drop,
            record_transcript: false,
            emit_operator_content: false,
            policy_version: PolicyVersion(1),
        };
        let json = serde_json::to_value(&decision).unwrap();
        let parsed: PolicyDecision = serde_json::from_value(json).unwrap();
        assert_eq!(decision, parsed);
    }

    #[test]
    fn record_transcript_defaults_true() {
        let json = serde_json::json!({
            "apply_mode": "stage_run_start",
            "wake_mode": "wake_if_idle",
            "queue_mode": "fifo",
            "consume_point": "on_run_complete",
            "policy_version": 1
        });
        let parsed: PolicyDecision = serde_json::from_value(json).unwrap();
        assert!(parsed.record_transcript);
        assert!(parsed.emit_operator_content);
    }
}
