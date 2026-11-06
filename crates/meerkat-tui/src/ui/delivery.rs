//! UI-side bookkeeping for messages typed while a turn is running.
//!
//! Each such message becomes a [`Pending`] item with a [`Block::Queued`]
//! placeholder in the transcript. The item moves through [`PendingState`] as
//! the runner admits it to the session's runtime ([`super::Msg::SteerAck`])
//! and the runtime writes it into the conversation
//! ([`super::Msg::SteerDelivered`], where the placeholder becomes a user
//! block in place). The state machine itself lives in [`super::App`]; this
//! module only owns the queue.
//!
//! Messages the UI still holds ([`PendingState::Deferred`]) are recalled into
//! the editor by the UI alone. Admitted messages come back through the runner
//! ([`super::Effect::RecallQueued`], answered by [`super::Msg::Recalled`]):
//! it abandons the ones the runtime still holds queued, leaving the running
//! turn alone; one that already joined the run stays, and only an interrupt
//! ends it.
//!
//! [`Block::Queued`]: super::transcript::Block::Queued

use crate::delivery::PendingId;

use super::transcript::{BlockId, QueuedLabel};

/// Where a pending message is in its delivery lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    /// Handed to the runner ([`super::Effect::Steer`]); no ack yet.
    Queued,
    /// Admitted to the runtime as a durable steer: written into the running
    /// turn at its next model boundary, or run as a follow-up turn.
    Admitted,
    /// Held by the UI (compaction is running, or no turn was running to
    /// steer into); sent as a new turn when the current work ends.
    Deferred,
}

impl PendingState {
    /// The label shown on the transcript placeholder.
    #[must_use]
    pub fn label(self) -> QueuedLabel {
        match self {
            Self::Queued => QueuedLabel::Queued,
            Self::Admitted => QueuedLabel::Delivering,
            Self::Deferred => QueuedLabel::Deferred,
        }
    }

    /// Whether the UI alone can pull the message back into the editor (Esc /
    /// ↑): only messages the runtime does not hold yet (admitted ones need a
    /// recall through the runner).
    #[must_use]
    pub fn recallable(self) -> bool {
        matches!(self, Self::Deferred)
    }
}

/// One message typed while a turn was running.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// Queue id (also the injection id).
    pub id: PendingId,
    /// Raw user text.
    pub text: String,
    /// Lifecycle state.
    pub state: PendingState,
    /// The [`Block::Queued`](super::transcript::Block::Queued) placeholder.
    pub block: BlockId,
}

/// Pending messages, oldest first.
#[derive(Debug, Clone, Default)]
pub struct PendingQueue {
    items: Vec<Pending>,
    /// Items already shown as delivered ([`super::Msg::SteerDelivered`])
    /// whose text is kept until the turn ends: a discarded application
    /// ([`super::Msg::SteerRequeued`]) or an undelivered terminal brings
    /// them back.
    settled: Vec<Pending>,
    next_id: u64,
}

impl PendingQueue {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate the next id.
    pub fn alloc_id(&mut self) -> PendingId {
        self.next_id += 1;
        PendingId(self.next_id)
    }

    /// The most recently allocated id (`PendingId(0)` before the first).
    #[must_use]
    pub fn last_id(&self) -> PendingId {
        PendingId(self.next_id)
    }

    /// Text of a queued or settled item.
    #[must_use]
    pub fn text_of(&self, id: PendingId) -> Option<&str> {
        self.all().find(|p| p.id == id).map(|p| p.text.as_str())
    }

    /// Every item the queue holds, queued first then settled.
    pub fn all(&self) -> impl Iterator<Item = &Pending> {
        self.items.iter().chain(self.settled.iter())
    }

    /// Append an item.
    pub fn push(&mut self, item: Pending) {
        self.items.push(item);
    }

    /// All items, oldest first.
    #[must_use]
    pub fn items(&self) -> &[Pending] {
        &self.items
    }

    /// Number of items not yet delivered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Item by id.
    #[must_use]
    pub fn get(&self, id: PendingId) -> Option<&Pending> {
        self.items.iter().find(|p| p.id == id)
    }

    /// Mutable item by id.
    pub fn get_mut(&mut self, id: PendingId) -> Option<&mut Pending> {
        self.items.iter_mut().find(|p| p.id == id)
    }

    /// Remove and return an item.
    pub fn remove(&mut self, id: PendingId) -> Option<Pending> {
        let idx = self.items.iter().position(|p| p.id == id)?;
        Some(self.items.remove(idx))
    }

    /// Remove every item matching `pred`, oldest first.
    pub fn take_where(&mut self, mut pred: impl FnMut(&Pending) -> bool) -> Vec<Pending> {
        let mut taken = Vec::new();
        let mut kept = Vec::with_capacity(self.items.len());
        for item in self.items.drain(..) {
            if pred(&item) {
                taken.push(item);
            } else {
                kept.push(item);
            }
        }
        self.items = kept;
        taken
    }

    /// Number of items in `state`-matching states.
    #[must_use]
    pub fn count(&self, pred: impl Fn(PendingState) -> bool) -> usize {
        self.items.iter().filter(|p| pred(p.state)).count()
    }

    /// The newest item that can still be recalled.
    #[must_use]
    pub fn newest_recallable(&self) -> Option<&Pending> {
        self.items.iter().rev().find(|p| p.state.recallable())
    }

    /// Move an item to the settled list (shown as delivered until the turn
    /// ends). Returns the settled item.
    pub fn settle(&mut self, id: PendingId) -> Option<&Pending> {
        let item = self.remove(id)?;
        self.settled.push(item);
        self.settled.last()
    }

    /// Remove and return a settled item.
    pub fn take_settled(&mut self, id: PendingId) -> Option<Pending> {
        let idx = self.settled.iter().position(|p| p.id == id)?;
        Some(self.settled.remove(idx))
    }

    /// Forget every settled item (the turn ended).
    pub fn clear_settled(&mut self) {
        self.settled.clear();
    }

    /// Fix up block ids after the transcript removed block `removed`.
    pub fn block_removed(&mut self, removed: BlockId) {
        for item in self.items.iter_mut().chain(self.settled.iter_mut()) {
            if item.block.0 > removed.0 {
                item.block = BlockId(item.block.0 - 1);
            }
        }
    }

    /// Mutable iteration, oldest first.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Pending> {
        self.items.iter_mut()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn item(q: &mut PendingQueue, text: &str, block: usize) -> PendingId {
        let id = q.alloc_id();
        q.push(Pending {
            id,
            text: text.into(),
            state: PendingState::Queued,
            block: BlockId(block),
        });
        id
    }

    #[test]
    fn ids_are_unique_and_lookup_works() {
        let mut q = PendingQueue::new();
        let a = item(&mut q, "a", 1);
        let b = item(&mut q, "b", 3);
        assert_ne!(a, b);
        assert_eq!(q.len(), 2);
        assert_eq!(q.get(b).map(|p| p.text.as_str()), Some("b"));
        q.get_mut(a).unwrap().state = PendingState::Deferred;
        assert_eq!(q.count(|s| s == PendingState::Deferred), 1);
        assert_eq!(q.remove(a).map(|p| p.text), Some("a".into()));
        assert!(q.get(a).is_none());
        // Ids keep increasing after removal.
        assert!(item(&mut q, "c", 4) > b);
    }

    #[test]
    fn only_deferred_items_are_recallable() {
        let mut q = PendingQueue::new();
        let a = item(&mut q, "a", 0);
        let b = item(&mut q, "b", 1);
        assert!(q.newest_recallable().is_none());
        q.get_mut(a).unwrap().state = PendingState::Deferred;
        assert_eq!(q.newest_recallable().map(|p| p.id), Some(a));
        q.get_mut(b).unwrap().state = PendingState::Deferred;
        assert_eq!(q.newest_recallable().map(|p| p.id), Some(b));
        q.get_mut(b).unwrap().state = PendingState::Admitted;
        assert_eq!(q.newest_recallable().map(|p| p.id), Some(a));
    }

    #[test]
    fn take_where_keeps_order_and_block_fixup_shifts() {
        let mut q = PendingQueue::new();
        let a = item(&mut q, "a", 2);
        let b = item(&mut q, "b", 5);
        let c = item(&mut q, "c", 7);
        let taken = q.take_where(|p| p.id != b);
        assert_eq!(taken.iter().map(|p| p.id).collect::<Vec<_>>(), vec![a, c]);
        assert_eq!(q.len(), 1);
        q.block_removed(BlockId(3));
        assert_eq!(q.get(b).unwrap().block, BlockId(4));
        q.block_removed(BlockId(9));
        assert_eq!(q.get(b).unwrap().block, BlockId(4));
        // Settled items leave the queue but keep their text and block fix-ups.
        assert_eq!(q.settle(b).map(|p| p.text.as_str()), Some("b"));
        assert!(q.is_empty());
        q.block_removed(BlockId(0));
        let settled = q.take_settled(b).unwrap();
        assert_eq!(settled.block, BlockId(3));
        assert!(q.take_settled(b).is_none());
    }

    #[test]
    fn labels_follow_states() {
        assert_eq!(PendingState::Queued.label(), QueuedLabel::Queued);
        assert_eq!(PendingState::Admitted.label(), QueuedLabel::Delivering);
        assert_eq!(PendingState::Deferred.label(), QueuedLabel::Deferred);
        assert!(PendingState::Deferred.recallable());
        assert!(!PendingState::Admitted.recallable());
        assert!(!PendingState::Queued.recallable());
    }
}
