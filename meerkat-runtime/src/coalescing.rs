//! §16 Coalescing — merging compatible inputs into aggregates.
//!
//! Only ExternalEventInput and PeerInput(ResponseProgress) are coalescing-eligible.
//! Supersession scoped by (LogicalRuntimeId, variant, SupersessionKey).
//! Cross-kind forbidden.

use chrono::Utc;
use meerkat_core::lifecycle::InputId;

use crate::identifiers::{LogicalRuntimeId, SupersessionKey};
use crate::input::{Input, PeerConvention};
use crate::input_state::{
    InputLifecycleState, InputState, InputStateHistoryEntry, InputTerminalOutcome,
};

/// Whether an input is eligible for coalescing.
pub fn is_coalescing_eligible(input: &Input) -> bool {
    matches!(
        input,
        Input::ExternalEvent(_)
            | Input::Peer(crate::input::PeerInput {
                convention: Some(PeerConvention::ResponseProgress { .. }),
                ..
            })
    )
}

/// Scope key for supersession — inputs with the same scope can supersede each other.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SupersessionScope {
    pub runtime_id: LogicalRuntimeId,
    pub kind: String,
    pub supersession_key: SupersessionKey,
}

impl SupersessionScope {
    /// Extract the supersession scope from an input (if it has a supersession key).
    pub fn from_input(input: &Input, runtime_id: &LogicalRuntimeId) -> Option<Self> {
        let key = input.header().supersession_key.as_ref()?;
        Some(Self {
            runtime_id: runtime_id.clone(),
            kind: input.kind_id().0,
            supersession_key: key.clone(),
        })
    }
}

/// Result of a coalescing check.
#[derive(Debug)]
pub enum CoalescingResult {
    /// No coalescing needed (input is standalone).
    Standalone,
    /// The input supersedes an existing input.
    Supersedes {
        /// ID of the superseded input.
        superseded_id: InputId,
    },
}

/// Check if a new input supersedes an existing input with the same scope.
pub fn check_supersession(
    new_input: &Input,
    existing_input: &Input,
    runtime_id: &LogicalRuntimeId,
) -> CoalescingResult {
    let new_scope = SupersessionScope::from_input(new_input, runtime_id);
    let existing_scope = SupersessionScope::from_input(existing_input, runtime_id);

    match (new_scope, existing_scope) {
        (Some(ns), Some(es)) if ns == es => {
            // Same scope — new supersedes existing
            CoalescingResult::Supersedes {
                superseded_id: existing_input.id().clone(),
            }
        }
        _ => CoalescingResult::Standalone,
    }
}

/// Write the shell-side metadata for a supersession. Callers invoke the DSL
/// `SupersedeInput` transition first (which flips `input_phases` and records
/// the structured terminal metadata in the typed terminal maps); this helper
/// keeps the shell's history log and typed outcome cache in step.
///
/// `from_phase` is the DSL-tracked lifecycle the input held at the moment the
/// DSL transition fired, captured by the caller so the history entry is
/// accurate.
pub fn apply_supersession(
    superseded_state: &mut InputState,
    from_phase: InputLifecycleState,
    superseded_by: InputId,
) {
    let now = Utc::now();
    superseded_state.history.push(InputStateHistoryEntry {
        timestamp: now,
        from: from_phase,
        to: InputLifecycleState::Superseded,
        reason: Some("Supersede".into()),
    });
    superseded_state.terminal_outcome = Some(InputTerminalOutcome::Superseded { superseded_by });
    superseded_state.updated_at = now;
}

/// Write the shell-side metadata for a coalesce. Callers invoke the DSL
/// `CoalesceInput` transition first; this helper keeps the shell's history
/// log and typed outcome cache in step.
pub fn apply_coalescing(
    source_state: &mut InputState,
    from_phase: InputLifecycleState,
    aggregate_id: InputId,
) {
    let now = Utc::now();
    source_state.history.push(InputStateHistoryEntry {
        timestamp: now,
        from: from_phase,
        to: InputLifecycleState::Coalesced,
        reason: Some("Coalesce".into()),
    });
    source_state.terminal_outcome = Some(InputTerminalOutcome::Coalesced { aggregate_id });
    source_state.updated_at = now;
}

/// Create an aggregate input from multiple coalesced inputs.
pub fn create_aggregate_input(
    sources: &[&Input],
    aggregate_id: InputId,
) -> Option<AggregateDescriptor> {
    if sources.is_empty() {
        return None;
    }

    let source_ids: Vec<InputId> = sources.iter().map(|i| i.id().clone()).collect();
    let summary = format!("{} coalesced inputs", sources.len());

    Some(AggregateDescriptor {
        aggregate_id,
        source_ids,
        summary,
        created_at: Utc::now(),
    })
}

/// Describes a coalesced aggregate (the caller creates the actual Input).
#[derive(Debug, Clone)]
pub struct AggregateDescriptor {
    pub aggregate_id: InputId,
    pub source_ids: Vec<InputId>,
    pub summary: String,
    pub created_at: chrono::DateTime<Utc>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;
    use meerkat_core::types::HandlingMode;

    fn make_header_with_supersession(key: Option<&str>) -> InputHeader {
        InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::External {
                source_name: "test".into(),
            },
            durability: InputDurability::Ephemeral,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: key.map(SupersessionKey::new),
            correlation_id: None,
        }
    }

    #[test]
    fn external_event_is_coalescing_eligible() {
        let input = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(None),
            event_type: "webhook".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        assert!(is_coalescing_eligible(&input));
    }

    #[test]
    fn response_progress_is_coalescing_eligible() {
        let input = Input::Peer(PeerInput {
            header: make_header_with_supersession(None),
            convention: Some(PeerConvention::ResponseProgress {
                request_id: "req-1".into(),
                phase: ResponseProgressPhase::InProgress,
            }),
            body: "progress".into(),
            payload: Some(serde_json::json!({"progress": "working"})),
            blocks: None,
            handling_mode: None,
        });
        assert!(is_coalescing_eligible(&input));
    }

    #[test]
    fn prompt_not_coalescing_eligible() {
        let input = Input::Prompt(PromptInput {
            header: make_header_with_supersession(None),
            text: "hello".into(),
            blocks: None,
            turn_metadata: None,
        });
        assert!(!is_coalescing_eligible(&input));
    }

    #[test]
    fn peer_message_not_coalescing_eligible() {
        let input = Input::Peer(PeerInput {
            header: make_header_with_supersession(None),
            convention: Some(PeerConvention::Message),
            body: "hello".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        });
        assert!(!is_coalescing_eligible(&input));
    }

    #[test]
    fn supersession_same_scope() {
        let runtime = LogicalRuntimeId::new("agent-1");
        let input1 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("status")),
            event_type: "status_update".into(),
            payload: serde_json::json!({"v": 1}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let input2 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("status")),
            event_type: "status_update".into(),
            payload: serde_json::json!({"v": 2}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let result = check_supersession(&input2, &input1, &runtime);
        assert!(matches!(result, CoalescingResult::Supersedes { .. }));
    }

    #[test]
    fn supersession_different_key() {
        let runtime = LogicalRuntimeId::new("agent-1");
        let input1 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("status-a")),
            event_type: "status_update".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let input2 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("status-b")),
            event_type: "status_update".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let result = check_supersession(&input2, &input1, &runtime);
        assert!(matches!(result, CoalescingResult::Standalone));
    }

    #[test]
    fn supersession_no_key() {
        let runtime = LogicalRuntimeId::new("agent-1");
        let input1 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(None),
            event_type: "status_update".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let input2 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(None),
            event_type: "status_update".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        let result = check_supersession(&input2, &input1, &runtime);
        assert!(matches!(result, CoalescingResult::Standalone));
    }

    #[test]
    fn cross_kind_supersession_forbidden() {
        let runtime = LogicalRuntimeId::new("agent-1");
        let input1 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("same-key")),
            event_type: "type-a".into(),
            payload: serde_json::json!({}),
            blocks: None,
            handling_mode: HandlingMode::Queue,
            render_metadata: None,
        });
        // Different kind (Prompt vs ExternalEvent) but same supersession key
        let input2 = Input::Prompt(PromptInput {
            header: make_header_with_supersession(Some("same-key")),
            text: "hello".into(),
            blocks: None,
            turn_metadata: None,
        });
        let result = check_supersession(&input2, &input1, &runtime);
        // Different kinds → different scope → no supersession
        assert!(matches!(result, CoalescingResult::Standalone));
    }

    #[test]
    fn apply_supersession_records_history_and_outcome() {
        let mut state = InputState::new_accepted(InputId::new());
        let superseder = InputId::new();
        apply_supersession(&mut state, InputLifecycleState::Queued, superseder);
        assert!(matches!(
            state.terminal_outcome.clone(),
            Some(InputTerminalOutcome::Superseded { .. })
        ));
        assert!(!state.history.is_empty());
        assert_eq!(
            state.history.last().map(|e| e.to),
            Some(InputLifecycleState::Superseded)
        );
    }

    #[test]
    fn apply_coalescing_records_history_and_outcome() {
        let mut state = InputState::new_accepted(InputId::new());
        let aggregate = InputId::new();
        apply_coalescing(&mut state, InputLifecycleState::Queued, aggregate);
        assert!(matches!(
            state.terminal_outcome.clone(),
            Some(InputTerminalOutcome::Coalesced { .. })
        ));
        assert!(!state.history.is_empty());
        assert_eq!(
            state.history.last().map(|e| e.to),
            Some(InputLifecycleState::Coalesced)
        );
    }

    #[test]
    fn create_aggregate_from_sources() {
        let sources: Vec<Input> = (0..3)
            .map(|_| {
                Input::ExternalEvent(ExternalEventInput {
                    header: make_header_with_supersession(None),
                    event_type: "test".into(),
                    payload: serde_json::json!({}),
                    blocks: None,
                    handling_mode: HandlingMode::Queue,
                    render_metadata: None,
                })
            })
            .collect();
        let source_refs: Vec<&Input> = sources.iter().collect();
        let agg = create_aggregate_input(&source_refs, InputId::new()).unwrap();
        assert_eq!(agg.source_ids.len(), 3);
        assert!(agg.summary.contains('3'));
    }

    #[test]
    fn create_aggregate_empty_returns_none() {
        let result = create_aggregate_input(&[], InputId::new());
        assert!(result.is_none());
    }
}
