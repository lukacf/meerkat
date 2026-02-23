use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Type-safe striped lock registry for wire edge operations.
///
/// Enforces the lock-ordering invariant internally: the registry lock
/// is held only for lookup/insert, never across edge operations.
pub(super) struct EdgeLockRegistry {
    locks: Mutex<BTreeMap<String, Arc<Mutex<()>>>>,
}

impl EdgeLockRegistry {
    pub(super) fn new() -> Self {
        Self {
            locks: Mutex::new(BTreeMap::new()),
        }
    }

    /// Acquire the edge lock for a canonically-ordered pair.
    /// Returns a guard that releases the lock on drop.
    pub(super) async fn acquire(&self, a: &str, b: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let key = Self::canonical_key(a, b);
        let lock = {
            let mut locks = self.locks.lock().await;
            locks
                .entry(key)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        // Outer lock released here — safe ordering.
        lock.lock_owned().await
    }

    /// Remove the edge lock for a specific pair.
    pub(super) async fn remove(&self, a: &str, b: &str) {
        let key = Self::canonical_key(a, b);
        self.locks.lock().await.remove(&key);
    }

    /// Remove all edge locks involving the given member.
    pub(super) async fn prune(&self, member_id: &str) {
        let mut locks = self.locks.lock().await;
        locks.retain(|key, _| !key.split('|').any(|part| part == member_id));
    }

    /// Remove all locks.
    pub(super) async fn clear(&self) {
        self.locks.lock().await.clear();
    }

    fn canonical_key(a: &str, b: &str) -> String {
        if a < b {
            format!("{a}|{b}")
        } else {
            format!("{b}|{a}")
        }
    }
}
