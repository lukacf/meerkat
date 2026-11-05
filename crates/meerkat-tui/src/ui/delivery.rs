//! UI-side bookkeeping for messages typed while a turn is running.
//!
//! Each such message becomes a [`Pending`] item with a [`Block::Queued`]
//! placeholder in the transcript. The item moves through [`PendingState`] as
//! the integrator acknowledges the injection, the model picks it up, the user
//! recalls it, or the post-turn [`DeliveryReport`](crate::delivery::DeliveryReport)
//! settles it. The state machine itself lives in [`super::App`]; this module
//! only owns the queue.
//!
//! [`Block::Queued`]: super::transcript::Block::Queued

use crate::delivery::PendingId;

use super::transcript::{BlockId, QueuedLabel};

/// Where a pending message is in its delivery lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    /// Handed to the integrator ([`super::Effect::Inject`]); no ack yet.
    Queued,
    /// In the agent's inbox. The next `TurnStarted` with a sequence number
    /// above `ack_seq` is taken as the model having seen it.
    Delivering {
        /// `App::last_seq` when the injection was acknowledged.
        ack_seq: u64,
    },
    /// Injection was refused; the message is sent after the turn ends.
    Deferred,
    /// The user asked for it back ([`super::Effect::RecallPending`]); waiting
    /// for [`super::Msg::Recalled`] or the post-turn report.
    Recalling,
}

impl PendingState {
    /// The label shown on the transcript placeholder.
    #[must_use]
    pub fn label(self) -> QueuedLabel {
        match self {
            Self::Queued => QueuedLabel::Queued,
            Self::Delivering { .. } => QueuedLabel::Delivering,
            Self::Deferred => QueuedLabel::Deferred,
            Self::Recalling => QueuedLabel::Recalling,
        }
    }

    /// Whether the user can still recall the message (Esc / ↑).
    #[must_use]
    pub fn recallable(self) -> bool {
        !matches!(self, Self::Recalling)
    }

    /// Whether recalling it needs the integrator to drain the inbox.
    #[must_use]
    pub fn in_inbox(self) -> bool {
        matches!(self, Self::Queued | Self::Delivering { .. })
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
    /// Items already shown as delivered (a `TurnStarted` followed their ack)
    /// whose text is kept until the post-turn report confirms delivery.
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

    /// Number of items (every item is not yet delivered).
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

    /// Move an item to the settled list (shown as delivered, awaiting the
    /// post-turn report). Returns the settled item.
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

    /// Forget every settled item (the report has been applied).
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
    fn newest_recallable_skips_recalling_items() {
        let mut q = PendingQueue::new();
        let a = item(&mut q, "a", 0);
        let b = item(&mut q, "b", 1);
        assert_eq!(q.newest_recallable().map(|p| p.id), Some(b));
        q.get_mut(b).unwrap().state = PendingState::Recalling;
        assert_eq!(q.newest_recallable().map(|p| p.id), Some(a));
        q.get_mut(a).unwrap().state = PendingState::Recalling;
        assert!(q.newest_recallable().is_none());
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
        assert_eq!(
            PendingState::Delivering { ack_seq: 3 }.label(),
            QueuedLabel::Delivering
        );
        assert_eq!(PendingState::Deferred.label(), QueuedLabel::Deferred);
        assert_eq!(PendingState::Recalling.label(), QueuedLabel::Recalling);
        assert!(PendingState::Queued.in_inbox());
        assert!(!PendingState::Deferred.in_inbox());
    }
}
