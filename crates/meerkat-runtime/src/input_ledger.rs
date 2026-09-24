//! InputLedger — in-memory ledger of InputState entries.
//!
//! IndexMap<InputId, InputState>.

use indexmap::IndexMap;
use meerkat_core::lifecycle::InputId;
use std::collections::BTreeSet;

use crate::input_state::InputState;

/// In-memory ledger tracking InputState for all inputs.
#[derive(Debug, Default, Clone)]
pub struct InputLedger {
    /// InputId → InputState (insertion order preserved).
    states: IndexMap<InputId, InputState>,
    /// Canonical owners of unfinished terminal work, ordered for stable paging.
    pending_terminal_owners: BTreeSet<uuid::Uuid>,
}

impl InputLedger {
    /// Create a new empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept a new InputState into the ledger.
    pub fn accept(&mut self, state: InputState) {
        let input_id = state.input_id.clone();
        self.states.insert(input_id.clone(), state);
        self.refresh_pending_terminal_owner(&input_id);
    }

    /// Recover an InputState after generated recovery authority retained it.
    ///
    /// Recovery retention and idempotency ownership live in the generated
    /// MeerkatMachine recovery/admission path, not in the ledger.
    /// Returns `true` if the state was inserted.
    pub fn recover(&mut self, state: InputState) -> bool {
        let input_id = state.input_id.clone();
        self.states.insert(input_id.clone(), state);
        self.refresh_pending_terminal_owner(&input_id);
        true
    }

    /// Get the state of a specific input.
    pub fn get(&self, input_id: &InputId) -> Option<&InputState> {
        self.states.get(input_id)
    }

    /// Remove an input from the ledger.
    pub fn remove(&mut self, input_id: &InputId) -> Option<InputState> {
        self.pending_terminal_owners.remove(&input_id.0);
        self.states.shift_remove(input_id)
    }

    /// Get mutable reference to the state of a specific input.
    pub fn get_mut(&mut self, input_id: &InputId) -> Option<&mut InputState> {
        self.states.get_mut(input_id)
    }

    /// Iterate over all input states. "Active" (non-terminal) filtering must
    /// happen at the driver level, which has DSL access; the ledger by itself
    /// carries only shell metadata.
    pub fn iter(&self) -> impl Iterator<Item = (&InputId, &InputState)> {
        self.states.iter()
    }

    /// Refresh the secondary pending-terminal owner index after an in-place
    /// shell mutation of one row.
    pub(crate) fn refresh_pending_terminal_owner(&mut self, input_id: &InputId) {
        let pending = self
            .states
            .get(input_id)
            .is_some_and(crate::store::input_state_is_pending_terminal_owner);
        if pending {
            self.pending_terminal_owners.insert(input_id.0);
        } else {
            self.pending_terminal_owners.remove(&input_id.0);
        }
    }

    /// Stable canonical owner ids for unfinished terminal work.
    pub(crate) fn pending_terminal_owner_ids(&self) -> Vec<InputId> {
        self.pending_terminal_owners
            .iter()
            .copied()
            .map(InputId::from_uuid)
            .collect()
    }

    /// Number of entries in the ledger.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Check if the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn accept_and_retrieve() {
        let mut ledger = InputLedger::new();
        let id = InputId::new();
        let state = InputState::new_accepted(id.clone());
        ledger.accept(state);

        assert_eq!(ledger.len(), 1);
        assert!(!ledger.is_empty());
        let retrieved = ledger.get(&id).unwrap();
        assert_eq!(retrieved.input_id, id);
    }

    #[test]
    fn accept_preserves_idempotency_key_as_metadata_only() {
        let mut ledger = InputLedger::new();

        let input_id = InputId::new();
        let mut state = InputState::new_accepted(input_id.clone());
        state.idempotency_key = Some(crate::identifiers::IdempotencyKey::new("req-123"));
        ledger.accept(state);

        assert_eq!(ledger.len(), 1);
        assert_eq!(
            ledger
                .get(&input_id)
                .and_then(|state| state.idempotency_key.as_ref()),
            Some(&crate::identifiers::IdempotencyKey::new("req-123"))
        );
    }

    #[test]
    fn recover_does_not_interpret_durability() {
        let mut ledger = InputLedger::new();

        let mut state = InputState::new_accepted(InputId::new());
        state.durability = Some(crate::input::InputDurability::Ephemeral);
        assert!(
            ledger.recover(state),
            "the ledger inserts rows after machine authority has made the retention decision"
        );
        assert_eq!(ledger.len(), 1);
    }
}
