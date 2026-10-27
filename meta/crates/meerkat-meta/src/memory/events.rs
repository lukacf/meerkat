//! Typed memory timeline events (§9.3).
//!
//! The memory plane emits its lifecycle onto the console timeline through
//! this seam: a sync, fire-and-forget sink so store/taint/guard code (which
//! is often inside mutexes or blocking threads) never awaits the event
//! surface. The wired implementation projects each event as a
//! `ConsoleIdentityEventEnvelope` (the standard console envelope):
//!
//! ```text
//! {
//!   "event_id":   "console-evt-<seq>"      // minted by the store
//!   "identity":   <affected identity, or "_system" for realm-level events>
//!   "event_type": "memory.<kind>"          // `MemoryTimelineEvent::event_type`
//!   "timestamp_ms": <now>,
//!   "data":       <`MemoryTimelineEvent::data`, the typed payload below>
//! }
//! ```
//!
//! P3b's console Memory panel consumes exactly this envelope; the
//! `event_type` strings and payload fields here are that contract. Sites
//! that cannot reach a wired sink keep their tracing warns — skipped work
//! stays loud either way (Principle 6).

use serde_json::{Value, json};

/// One typed memory-plane timeline event. Payload fields are stable wire
/// contract for the console Memory panel (P3b).
#[derive(Debug, Clone, PartialEq)]
pub enum MemoryTimelineEvent {
    /// A steward dream run started.
    DreamStarted { realm: String, run_id: String },
    /// A steward dream run completed; `detail` carries the run summary
    /// (phase outcomes, verdict counts).
    DreamCompleted {
        realm: String,
        run_id: String,
        ops_committed: usize,
        detail: Value,
    },
    /// A steward dream run was skipped (gates, budget, lock, errors).
    DreamSkipped { realm: String, reason: String },
    /// A record was promoted into a wider scope (identity→mob), either
    /// directly by a dream commit or through an approved gate.
    RecordPromoted {
        realm: String,
        record_id: String,
        source_record_id: Option<String>,
        scope_kind: String,
        scope_key: String,
        proposal_id: Option<String>,
        gated: bool,
    },
    /// A quarantine verdict on a record. A review transaction emits it once
    /// its decision has committed, with `successor_id` naming a release's
    /// successor; who decided and why stay in the review's protected audit
    /// rows and receipt, never on this system-wide timeline.
    QuarantineVerdict {
        realm: String,
        record_id: String,
        verdict: String,
        rationale: Option<String>,
        successor_id: Option<String>,
    },
    /// A quarantine release/promotion verdict was blocked before staging:
    /// the record's content matches a §10.4 secret pattern class, which
    /// the staged chokepoint refuses. The record stays quarantined — this
    /// event is why the queue never drains it (tombstone is the exit).
    QuarantineReleaseBlocked {
        realm: String,
        record_id: String,
        verdict: String,
        class: String,
    },
    /// A dream contradiction finding with operational consequence was
    /// bridged into the operational ledger (§8.5).
    ConflictSignal {
        realm: String,
        entity: String,
        topic: String,
        reason: String,
    },
    /// An LLM-authored write landed quarantined at the store seam (§10.1).
    QuarantinedWrite {
        realm: String,
        author: String,
        reason: String,
    },
    /// A session-taint transition (§10.1): `kind` is one of `tainted`,
    /// `reset_boundary`, `rotated_clean`.
    TaintTransition {
        identity: Option<String>,
        session_key: String,
        kind: String,
        source: String,
    },
    /// A background-budget guard denied a run (§8.1).
    BudgetDenied {
        realm: String,
        stage: String,
        reason: String,
    },
    /// A quarantine-promotion was staged and now awaits operator approval
    /// through the gating flow (§10.2).
    PromotionPendingGate {
        realm: String,
        pending_id: String,
        record_id: String,
        scope_kind: String,
        scope_key: String,
    },
    /// An exit-interview harvest of a retired identity's store completed.
    HarvestCompleted {
        realm: String,
        identity: String,
        promoted: usize,
        tombstoned: usize,
    },
    /// A pre-rotation distillation timed out and rotation proceeded
    /// without it (§8.4).
    DistillationTimedOut {
        identity: String,
        session_key: String,
        cause: String,
    },
    /// A hygiene pass produced a validated revision proposal (§8.6). The
    /// audited apply follows as `HygieneApplied` unless the seam refuses
    /// (e.g. the session is mid-turn).
    HygieneProposed {
        identity: String,
        session_key: String,
        cause: String,
        ops: usize,
        /// Active records whose evidence spans the revision touches — the
        /// §8.6 audit flag, allowed but on the record.
        flagged_active_records: Vec<String>,
    },
    /// An audited transcript revision committed (§8.6).
    HygieneApplied {
        identity: String,
        session_key: String,
        cause: String,
        parent_revision: String,
        revision: String,
        ops: usize,
        flagged_active_records: Vec<String>,
    },
    /// The §8.6 validator refused a revision (quarantine-referenced span,
    /// ordering invariant unmet, malformed ranges).
    HygieneBlocked {
        identity: String,
        session_key: String,
        cause: String,
        reason: String,
    },
    /// A hygiene pass was skipped (budget, no-op judgment, seam refusal).
    HygieneSkipped {
        identity: String,
        session_key: String,
        cause: String,
        reason: String,
    },
}

impl MemoryTimelineEvent {
    /// Stable `event_type` for the console envelope.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::DreamStarted { .. } => "memory.dream.started",
            Self::DreamCompleted { .. } => "memory.dream.completed",
            Self::DreamSkipped { .. } => "memory.dream.skipped",
            Self::RecordPromoted { .. } => "memory.record.promoted",
            Self::QuarantineVerdict { .. } => "memory.quarantine.verdict",
            Self::QuarantineReleaseBlocked { .. } => "memory.quarantine.release_blocked",
            Self::ConflictSignal { .. } => "memory.conflict.signal",
            Self::QuarantinedWrite { .. } => "memory.write.quarantined",
            Self::TaintTransition { .. } => "memory.taint.transition",
            Self::BudgetDenied { .. } => "memory.budget.denied",
            Self::PromotionPendingGate { .. } => "memory.promotion.pending_gate",
            Self::HarvestCompleted { .. } => "memory.harvest.completed",
            Self::DistillationTimedOut { .. } => "memory.distill.timed_out",
            Self::HygieneProposed { .. } => "memory.hygiene.proposed",
            Self::HygieneApplied { .. } => "memory.hygiene.applied",
            Self::HygieneBlocked { .. } => "memory.hygiene.blocked",
            Self::HygieneSkipped { .. } => "memory.hygiene.skipped",
        }
    }

    /// Console identity attribution: the affected identity where one
    /// exists, otherwise `None` (the sink attributes to the system
    /// identity).
    pub fn identity(&self) -> Option<&str> {
        match self {
            Self::TaintTransition { identity, .. } => identity.as_deref(),
            Self::HarvestCompleted { identity, .. }
            | Self::DistillationTimedOut { identity, .. }
            | Self::HygieneProposed { identity, .. }
            | Self::HygieneApplied { identity, .. }
            | Self::HygieneBlocked { identity, .. }
            | Self::HygieneSkipped { identity, .. } => Some(identity),
            _ => None,
        }
    }

    /// Typed payload for the console envelope's `data`.
    pub fn data(&self) -> Value {
        match self {
            Self::DreamStarted { realm, run_id } => json!({
                "realm": realm,
                "run_id": run_id,
            }),
            Self::DreamCompleted {
                realm,
                run_id,
                ops_committed,
                detail,
            } => json!({
                "realm": realm,
                "run_id": run_id,
                "ops_committed": ops_committed,
                "detail": detail,
            }),
            Self::DreamSkipped { realm, reason } => json!({
                "realm": realm,
                "reason": reason,
            }),
            Self::RecordPromoted {
                realm,
                record_id,
                source_record_id,
                scope_kind,
                scope_key,
                proposal_id,
                gated,
            } => json!({
                "realm": realm,
                "record_id": record_id,
                "source_record_id": source_record_id,
                "scope_kind": scope_kind,
                "scope_key": scope_key,
                "proposal_id": proposal_id,
                "gated": gated,
            }),
            Self::QuarantineVerdict {
                realm,
                record_id,
                verdict,
                rationale,
                successor_id,
            } => {
                let mut data = json!({
                    "realm": realm,
                    "record_id": record_id,
                    "verdict": verdict,
                    "rationale": rationale,
                });
                // Added only when present, so verdicts without a successor
                // keep their payload shape.
                if let (Some(successor_id), Some(object)) = (successor_id, data.as_object_mut()) {
                    object.insert("successor_id".to_string(), json!(successor_id));
                }
                data
            }
            Self::QuarantineReleaseBlocked {
                realm,
                record_id,
                verdict,
                class,
            } => json!({
                "realm": realm,
                "record_id": record_id,
                "verdict": verdict,
                "class": class,
            }),
            Self::ConflictSignal {
                realm,
                entity,
                topic,
                reason,
            } => json!({
                "realm": realm,
                "entity": entity,
                "topic": topic,
                "reason": reason,
            }),
            Self::QuarantinedWrite {
                realm,
                author,
                reason,
            } => json!({
                "realm": realm,
                "author": author,
                "reason": reason,
            }),
            Self::TaintTransition {
                identity,
                session_key,
                kind,
                source,
            } => json!({
                "identity": identity,
                "session_key": session_key,
                "kind": kind,
                "source": source,
            }),
            Self::BudgetDenied {
                realm,
                stage,
                reason,
            } => json!({
                "realm": realm,
                "stage": stage,
                "reason": reason,
            }),
            Self::PromotionPendingGate {
                realm,
                pending_id,
                record_id,
                scope_kind,
                scope_key,
            } => json!({
                "realm": realm,
                "pending_id": pending_id,
                "record_id": record_id,
                "scope_kind": scope_kind,
                "scope_key": scope_key,
            }),
            Self::HarvestCompleted {
                realm,
                identity,
                promoted,
                tombstoned,
            } => json!({
                "realm": realm,
                "identity": identity,
                "promoted": promoted,
                "tombstoned": tombstoned,
            }),
            Self::DistillationTimedOut {
                identity,
                session_key,
                cause,
            } => json!({
                "identity": identity,
                "session_key": session_key,
                "cause": cause,
            }),
            Self::HygieneProposed {
                identity,
                session_key,
                cause,
                ops,
                flagged_active_records,
            } => json!({
                "identity": identity,
                "session_key": session_key,
                "cause": cause,
                "ops": ops,
                "flagged_active_records": flagged_active_records,
            }),
            Self::HygieneApplied {
                identity,
                session_key,
                cause,
                parent_revision,
                revision,
                ops,
                flagged_active_records,
            } => json!({
                "identity": identity,
                "session_key": session_key,
                "cause": cause,
                "parent_revision": parent_revision,
                "revision": revision,
                "ops": ops,
                "flagged_active_records": flagged_active_records,
            }),
            Self::HygieneBlocked {
                identity,
                session_key,
                cause,
                reason,
            } => json!({
                "identity": identity,
                "session_key": session_key,
                "cause": cause,
                "reason": reason,
            }),
            Self::HygieneSkipped {
                identity,
                session_key,
                cause,
                reason,
            } => json!({
                "identity": identity,
                "session_key": session_key,
                "cause": cause,
                "reason": reason,
            }),
        }
    }
}

/// Fire-and-forget emission seam. Implementations must not block: the
/// wired console sink spawns the async append onto the runtime.
pub trait MemoryEventSink: Send + Sync {
    fn emit(&self, event: MemoryTimelineEvent);
}

/// Test helper: collects emitted events behind a mutex.
#[cfg(test)]
pub(crate) struct CollectingEventSink {
    pub events: std::sync::Mutex<Vec<MemoryTimelineEvent>>,
}

#[cfg(test)]
impl CollectingEventSink {
    pub(crate) fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn types(&self) -> Vec<&'static str> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(MemoryTimelineEvent::event_type)
            .collect()
    }
}

#[cfg(test)]
impl MemoryEventSink for CollectingEventSink {
    fn emit(&self, event: MemoryTimelineEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_are_stable_wire_contract() {
        let event = MemoryTimelineEvent::DreamSkipped {
            realm: "family".to_string(),
            reason: "budget".to_string(),
        };
        assert_eq!(event.event_type(), "memory.dream.skipped");
        assert_eq!(event.data(), json!({"realm": "family", "reason": "budget"}));
        assert_eq!(event.identity(), None);

        let event = MemoryTimelineEvent::TaintTransition {
            identity: Some("identity:luka".to_string()),
            session_key: "sess-1".to_string(),
            kind: "tainted".to_string(),
            source: "mcp:web".to_string(),
        };
        assert_eq!(event.event_type(), "memory.taint.transition");
        assert_eq!(event.identity(), Some("identity:luka"));
    }
}
