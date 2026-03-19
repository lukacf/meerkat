//! §16 Coalescing — merging compatible inputs into aggregates.
//!
//! Only ExternalEventInput and PeerInput(ResponseProgress) are coalescing-eligible.
//! Supersession scoped by (LogicalRuntimeId, variant, SupersessionKey).
//! Cross-kind forbidden.

use chrono::Utc;
use meerkat_core::lifecycle::InputId;

use crate::identifiers::{LogicalRuntimeId, SupersessionKey};
use crate::input::{Input, PeerConvention};
use crate::input_lifecycle_authority::{InputLifecycleError, InputLifecycleInput};
use crate::input_state::{InputState, InputTerminalOutcome};

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

/// Apply supersession: transition the superseded input to Superseded.
pub fn apply_supersession(
    superseded_state: &mut InputState,
    superseded_by: InputId,
) -> Result<(), InputLifecycleError> {
    superseded_state.apply(InputLifecycleInput::Supersede)?;
    superseded_state.set_terminal_outcome(InputTerminalOutcome::Superseded { superseded_by });
    Ok(())
}

/// Apply coalescing: transition the source input to Coalesced.
pub fn apply_coalescing(
    source_state: &mut InputState,
    aggregate_id: InputId,
) -> Result<(), InputLifecycleError> {
    source_state.apply(InputLifecycleInput::Coalesce)?;
    source_state.set_terminal_outcome(InputTerminalOutcome::Coalesced { aggregate_id });
    Ok(())
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
            blocks: None,
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
            blocks: None,
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
        });
        let input2 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("status")),
            event_type: "status_update".into(),
            payload: serde_json::json!({"v": 2}),
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
        });
        let input2 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(Some("status-b")),
            event_type: "status_update".into(),
            payload: serde_json::json!({}),
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
        });
        let input2 = Input::ExternalEvent(ExternalEventInput {
            header: make_header_with_supersession(None),
            event_type: "status_update".into(),
            payload: serde_json::json!({}),
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
    fn apply_supersession_transitions_state() {
        let mut state = InputState::new_accepted(InputId::new());
        state
            .apply(crate::input_lifecycle_authority::InputLifecycleInput::QueueAccepted)
            .unwrap();
        let superseder = InputId::new();
        apply_supersession(&mut state, superseder).unwrap();
        assert_eq!(
            state.current_state(),
            crate::input_state::InputLifecycleState::Superseded
        );
        assert!(matches!(
            state.terminal_outcome().cloned(),
            Some(InputTerminalOutcome::Superseded { .. })
        ));
    }

    #[test]
    fn apply_coalescing_transitions_state() {
        let mut state = InputState::new_accepted(InputId::new());
        state
            .apply(crate::input_lifecycle_authority::InputLifecycleInput::QueueAccepted)
            .unwrap();
        let aggregate = InputId::new();
        apply_coalescing(&mut state, aggregate).unwrap();
        assert_eq!(
            state.current_state(),
            crate::input_state::InputLifecycleState::Coalesced
        );
        assert!(matches!(
            state.terminal_outcome().cloned(),
            Some(InputTerminalOutcome::Coalesced { .. })
        ));
    }

    #[test]
    fn create_aggregate_from_sources() {
        let sources: Vec<Input> = (0..3)
            .map(|_| {
                Input::ExternalEvent(ExternalEventInput {
                    header: make_header_with_supersession(None),
                    event_type: "test".into(),
                    payload: serde_json::json!({}),
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
