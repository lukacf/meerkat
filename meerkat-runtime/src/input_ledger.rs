//! InputLedger — in-memory ledger of InputState entries.
//!
//! IndexMap<InputId, InputState> with dedup by idempotency_key.

use indexmap::IndexMap;
use meerkat_core::lifecycle::InputId;

use crate::identifiers::IdempotencyKey;
use crate::input::InputDurability;
use crate::input_state::InputState;

/// In-memory ledger tracking InputState for all inputs.
#[derive(Debug, Default)]
pub struct InputLedger {
    /// InputId → InputState (insertion order preserved).
    states: IndexMap<InputId, InputState>,
    /// IdempotencyKey → InputId for dedup lookup.
    idempotency_index: IndexMap<IdempotencyKey, InputId>,
}

impl InputLedger {
    /// Create a new empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Accept a new InputState into the ledger.
    pub fn accept(&mut self, state: InputState) {
        self.states.insert(state.input_id.clone(), state);
    }

    /// Accept with an idempotency key for dedup.
    /// Returns `Some(existing_id)` if the key already exists (dedup hit).
    pub fn accept_with_idempotency(
        &mut self,
        state: InputState,
        key: IdempotencyKey,
    ) -> Option<InputId> {
        if let Some(existing_id) = self.idempotency_index.get(&key) {
            return Some(existing_id.clone());
        }
        let input_id = state.input_id.clone();
        self.idempotency_index.insert(key, input_id);
        self.states.insert(state.input_id.clone(), state);
        None
    }

    /// Recover a durable InputState from persistent storage.
    ///
    /// Unlike `accept()`, this also rebuilds the idempotency index
    /// and filters out Ephemeral inputs (which should not survive restart).
    /// Returns `true` if the state was inserted, `false` if filtered.
    pub fn recover(&mut self, state: InputState) -> bool {
        // Ephemeral inputs should not survive restarts
        if state.durability == Some(InputDurability::Ephemeral) {
            return false;
        }

        // Rebuild idempotency index so dedup works after restart
        if let Some(ref key) = state.idempotency_key {
            self.idempotency_index
                .insert(key.clone(), state.input_id.clone());
        }

        self.states.insert(state.input_id.clone(), state);
        true
    }

    /// Get the state of a specific input.
    pub fn get(&self, input_id: &InputId) -> Option<&InputState> {
        self.states.get(input_id)
    }

    /// Get mutable reference to the state of a specific input.
    pub fn get_mut(&mut self, input_id: &InputId) -> Option<&mut InputState> {
        self.states.get_mut(input_id)
    }

    /// Iterate over all non-terminal input states.
    pub fn iter_non_terminal(&self) -> impl Iterator<Item = (&InputId, &InputState)> {
        self.states.iter().filter(|(_, s)| !s.is_terminal())
    }

    /// Iterate over all input states.
    pub fn iter(&self) -> impl Iterator<Item = (&InputId, &InputState)> {
        self.states.iter()
    }

    /// Number of entries in the ledger.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Check if the ledger is empty.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Number of non-terminal entries.
    pub fn active_count(&self) -> usize {
        self.states.values().filter(|s| !s.is_terminal()).count()
    }

    /// Get all active (non-terminal) input IDs.
    pub fn active_input_ids(&self) -> Vec<InputId> {
        self.iter_non_terminal().map(|(id, _)| id.clone()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input_machine::InputStateMachine;
    use crate::input_state::InputLifecycleState;

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
    fn dedup_by_idempotency_key() {
        let mut ledger = InputLedger::new();
        let key = IdempotencyKey::new("req-123");

        let id1 = InputId::new();
        let state1 = InputState::new_accepted(id1.clone());
        let result = ledger.accept_with_idempotency(state1, key.clone());
        assert!(result.is_none()); // First time — accepted

        let id2 = InputId::new();
        let state2 = InputState::new_accepted(id2);
        let result = ledger.accept_with_idempotency(state2, key);
        assert!(result.is_some()); // Duplicate — returns existing ID
        assert_eq!(result.unwrap(), id1);
        assert_eq!(ledger.len(), 1); // Only one entry
    }

    #[test]
    fn iter_non_terminal() {
        let mut ledger = InputLedger::new();

        let id1 = InputId::new();
        let state1 = InputState::new_accepted(id1.clone());
        ledger.accept(state1);

        let id2 = InputId::new();
        let mut state2 = InputState::new_accepted(id2);
        InputStateMachine::transition(&mut state2, InputLifecycleState::Consumed, None).unwrap();
        ledger.accept(state2);

        let active: Vec<_> = ledger.iter_non_terminal().collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].0, &id1);
    }

    #[test]
    fn active_count() {
        let mut ledger = InputLedger::new();

        ledger.accept(InputState::new_accepted(InputId::new()));
        ledger.accept(InputState::new_accepted(InputId::new()));

        let id3 = InputId::new();
        let mut state3 = InputState::new_accepted(id3);
        InputStateMachine::transition(&mut state3, InputLifecycleState::Superseded, None).unwrap();
        ledger.accept(state3);

        assert_eq!(ledger.len(), 3);
        assert_eq!(ledger.active_count(), 2);
    }

    #[test]
    fn active_input_ids() {
        let mut ledger = InputLedger::new();
        let id1 = InputId::new();
        let id2 = InputId::new();
        ledger.accept(InputState::new_accepted(id1.clone()));
        ledger.accept(InputState::new_accepted(id2.clone()));

        let ids = ledger.active_input_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn recover_rebuilds_idempotency_index() {
        let mut ledger = InputLedger::new();
        let key = IdempotencyKey::new("req-123");

        // Simulate recovery: inject state with an idempotency key
        let id1 = InputId::new();
        let mut state = InputState::new_accepted(id1.clone());
        state.idempotency_key = Some(key.clone());
        state.durability = Some(InputDurability::Durable);
        assert!(ledger.recover(state));

        // Now try to accept a new input with the same key → should be dedup'd
        let id2 = InputId::new();
        let state2 = InputState::new_accepted(id2);
        let result = ledger.accept_with_idempotency(state2, key);
        assert_eq!(result, Some(id1), "Dedup should find the recovered input");
        assert_eq!(ledger.len(), 1, "No duplicate entry should be created");
    }

    #[test]
    fn recover_filters_ephemeral() {
        let mut ledger = InputLedger::new();

        // Ephemeral input should be filtered out during recovery
        let mut state = InputState::new_accepted(InputId::new());
        state.durability = Some(InputDurability::Ephemeral);
        assert!(
            !ledger.recover(state),
            "Ephemeral inputs should be filtered"
        );
        assert!(ledger.is_empty());

        // Durable input should be kept
        let mut state = InputState::new_accepted(InputId::new());
        state.durability = Some(InputDurability::Durable);
        assert!(ledger.recover(state), "Durable inputs should be kept");
        assert_eq!(ledger.len(), 1);
    }
}
