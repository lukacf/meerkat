//! InputQueue — FIFO queue with scope filtering.

use std::collections::VecDeque;

use meerkat_core::lifecycle::InputId;

use crate::input::Input;

/// A queued input entry.
#[derive(Debug, Clone)]
pub struct QueuedInput {
    pub input_id: InputId,
    pub input: Input,
}

/// FIFO input queue.
#[derive(Debug, Default, Clone)]
pub struct InputQueue {
    queue: VecDeque<QueuedInput>,
}

impl InputQueue {
    /// Create a new empty queue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue an input.
    pub fn enqueue(&mut self, input_id: InputId, input: Input) {
        self.queue.push_back(QueuedInput { input_id, input });
    }

    /// Enqueue an input at the front of the queue.
    pub fn enqueue_front(&mut self, input_id: InputId, input: Input) {
        self.queue.push_front(QueuedInput { input_id, input });
    }

    /// Dequeue the next input (FIFO).
    pub fn dequeue(&mut self) -> Option<QueuedInput> {
        self.queue.pop_front()
    }

    /// Peek at the next input without removing it.
    pub fn peek(&self) -> Option<&QueuedInput> {
        self.queue.front()
    }

    /// Check if the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Number of entries in the queue.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Remove a specific input by ID and return it as (InputId, Input).
    ///
    /// Used by the batching policy to dequeue specific IDs determined by the authority.
    pub fn dequeue_by_id(&mut self, input_id: &InputId) -> Option<(InputId, crate::input::Input)> {
        self.remove(input_id).map(|q| (q.input_id, q.input))
    }

    /// Remove a specific input by ID (e.g., for supersession).
    pub fn remove(&mut self, input_id: &InputId) -> Option<QueuedInput> {
        if let Some(pos) = self.queue.iter().position(|q| q.input_id == *input_id) {
            self.queue.remove(pos)
        } else {
            None
        }
    }

    /// Drain all entries from the queue.
    pub fn drain(&mut self) -> Vec<QueuedInput> {
        self.queue.drain(..).collect()
    }

    /// Get all input IDs in the queue (in order).
    pub fn input_ids(&self) -> Vec<InputId> {
        self.queue.iter().map(|q| q.input_id.clone()).collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input::*;
    use chrono::Utc;

    fn make_prompt(id: InputId) -> Input {
        Input::Prompt(PromptInput {
            header: InputHeader {
                id,
                timestamp: Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Ephemeral,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            text: "test".into(),
            blocks: None,
            turn_metadata: None,
        })
    }

    #[test]
    fn fifo_ordering() {
        let mut queue = InputQueue::new();
        let id1 = InputId::new();
        let id2 = InputId::new();
        let id3 = InputId::new();

        queue.enqueue(id1.clone(), make_prompt(id1.clone()));
        queue.enqueue(id2.clone(), make_prompt(id2.clone()));
        queue.enqueue(id3.clone(), make_prompt(id3.clone()));

        assert_eq!(queue.len(), 3);
        assert_eq!(queue.dequeue().unwrap().input_id, id1);
        assert_eq!(queue.dequeue().unwrap().input_id, id2);
        assert_eq!(queue.dequeue().unwrap().input_id, id3);
        assert!(queue.is_empty());
    }

    #[test]
    fn remove_by_id() {
        let mut queue = InputQueue::new();
        let id1 = InputId::new();
        let id2 = InputId::new();

        queue.enqueue(id1.clone(), make_prompt(id1.clone()));
        queue.enqueue(id2.clone(), make_prompt(id2.clone()));

        let removed = queue.remove(&id1);
        assert!(removed.is_some());
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.dequeue().unwrap().input_id, id2);
    }

    #[test]
    fn remove_nonexistent() {
        let mut queue = InputQueue::new();
        let result = queue.remove(&InputId::new());
        assert!(result.is_none());
    }

    #[test]
    fn peek_does_not_remove() {
        let mut queue = InputQueue::new();
        let id = InputId::new();
        queue.enqueue(id.clone(), make_prompt(id.clone()));

        assert_eq!(queue.peek().unwrap().input_id, id);
        assert_eq!(queue.len(), 1); // Still there
    }

    #[test]
    fn drain_empties_queue() {
        let mut queue = InputQueue::new();
        queue.enqueue(InputId::new(), make_prompt(InputId::new()));
        queue.enqueue(InputId::new(), make_prompt(InputId::new()));

        let drained = queue.drain();
        assert_eq!(drained.len(), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn enqueue_front_wins_ordering() {
        let mut queue = InputQueue::new();
        let back = InputId::new();
        let front = InputId::new();

        queue.enqueue(back.clone(), make_prompt(back.clone()));
        queue.enqueue_front(front.clone(), make_prompt(front.clone()));

        assert_eq!(queue.dequeue().unwrap().input_id, front);
        assert_eq!(queue.dequeue().unwrap().input_id, back);
    }
}
