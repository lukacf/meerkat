//! Bundled single-process LeaseProvider for `persistent_state(path)` usage.
//!
//! Implements CONTRACT-07. **Single-process only**: assumes one active runtime
//! per `persistent_state` directory. Multi-pod or failover deployments MUST
//! supply an external `LeaseProvider`.
//!
//! FencingTokens are monotonic within the local store. Leases are tracked
//! in-memory — there is no TTL enforcement (the single-process assumption
//! makes TTL-based expiry unnecessary).

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use super::contracts::LeaseProvider;
use super::types::{
    AgentIdentity, FencingToken, LeaseAcquireResult, LeaseError, LeaseGrant, LeaseRenewResult,
};

/// Default TTL returned by the local lease provider.
///
/// The provider is single-process and does not expire records internally, so
/// this is a generous local ownership window rather than a distributed lease
/// deadline. External lease providers should return their real coordination TTL.
const LOCAL_LEASE_TTL: Duration = Duration::from_hours(24);

struct LeaseRecord {
    fencing_token: FencingToken,
    holder: String,
}

/// Single-process `LeaseProvider` for the bundled `persistent_state(path)` path.
///
/// **Single-process only.** Assumes one active runtime per persistent_state
/// directory. Does not provide distributed coordination. Multi-pod or failover
/// deployments must supply an external `LeaseProvider`.
pub struct LocalLeaseProvider {
    state: Mutex<LocalLeaseState>,
}

struct LocalLeaseState {
    next_token: u64,
    leases: BTreeMap<AgentIdentity, LeaseRecord>,
}

impl LocalLeaseProvider {
    /// Create a new local lease provider whose fencing counter starts at 1.
    #[must_use]
    pub fn new() -> Self {
        Self::with_floor(0)
    }

    /// Create a local lease provider whose monotonic fencing counter resumes
    /// strictly above `floor` (the persisted high-water mark), so fencing tokens
    /// keep advancing across process restarts instead of resetting to 1.
    ///
    /// Seed `floor` from
    /// [`LocalContinuityStore::max_fencing_token`](super::local_store::LocalContinuityStore::max_fencing_token)
    /// at startup. Without this, a restart re-issues token 1 and restore presents
    /// a stale token that the store's compare-and-set rejects (the v0.7.8
    /// "stale fencing token: presented 1, current N" restart abort).
    #[must_use]
    pub fn with_floor(floor: u64) -> Self {
        Self {
            state: Mutex::new(LocalLeaseState {
                next_token: floor.saturating_add(1),
                leases: BTreeMap::new(),
            }),
        }
    }
}

impl Default for LocalLeaseProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LeaseProvider for LocalLeaseProvider {
    async fn acquire_leases(
        &self,
        identities: &[AgentIdentity],
        runtime_instance: &str,
    ) -> Result<BTreeMap<AgentIdentity, LeaseAcquireResult>, LeaseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| LeaseError::Io(format!("lock: {e}")))?;
        let mut results = BTreeMap::new();
        for id in identities {
            if let Some(record) = state.leases.get(id)
                && record.holder != runtime_instance
            {
                results.insert(
                    id.clone(),
                    LeaseAcquireResult::AlreadyHeld {
                        identity: id.clone(),
                        holder: record.holder.clone(),
                    },
                );
                continue;
            }
            let token = FencingToken::new(state.next_token);
            state.next_token += 1;
            state.leases.insert(
                id.clone(),
                LeaseRecord {
                    fencing_token: token,
                    holder: runtime_instance.to_string(),
                },
            );
            results.insert(
                id.clone(),
                LeaseAcquireResult::Acquired(LeaseGrant {
                    identity: id.clone(),
                    fencing_token: token,
                    ttl: LOCAL_LEASE_TTL,
                }),
            );
        }
        Ok(results)
    }

    async fn renew_leases(
        &self,
        grants: &[LeaseGrant],
    ) -> Result<BTreeMap<AgentIdentity, LeaseRenewResult>, LeaseError> {
        let state = self
            .state
            .lock()
            .map_err(|e| LeaseError::Io(format!("lock: {e}")))?;
        let mut results = BTreeMap::new();
        for grant in grants {
            match state.leases.get(&grant.identity) {
                Some(record) if record.fencing_token == grant.fencing_token => {
                    results.insert(
                        grant.identity.clone(),
                        LeaseRenewResult::Renewed(LeaseGrant {
                            identity: grant.identity.clone(),
                            fencing_token: record.fencing_token,
                            ttl: LOCAL_LEASE_TTL,
                        }),
                    );
                }
                _ => {
                    results.insert(
                        grant.identity.clone(),
                        LeaseRenewResult::Lost {
                            identity: grant.identity.clone(),
                        },
                    );
                }
            }
        }
        Ok(results)
    }

    async fn release_leases(&self, grants: &[LeaseGrant]) -> Result<(), LeaseError> {
        let mut state = self
            .state
            .lock()
            .map_err(|e| LeaseError::Io(format!("lock: {e}")))?;
        for grant in grants {
            if let Some(record) = state.leases.get(&grant.identity)
                && record.fencing_token == grant.fencing_token
            {
                state.leases.remove(&grant.identity);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    async fn first_token(provider: &LocalLeaseProvider, holder: &str) -> u64 {
        let id = AgentIdentity::parse("identity:lead-1").unwrap();
        let result = provider
            .acquire_leases(std::slice::from_ref(&id), holder)
            .await
            .unwrap();
        match result.get(&id) {
            Some(LeaseAcquireResult::Acquired(grant)) => grant.fencing_token.get(),
            _ => panic!("expected an acquired lease"),
        }
    }

    #[tokio::test]
    async fn new_starts_the_fencing_counter_at_one() {
        assert_eq!(first_token(&LocalLeaseProvider::new(), "rt").await, 1);
    }

    #[tokio::test]
    async fn with_floor_resumes_strictly_above_the_persisted_high_water() {
        // Restart regression: seeded from a high-water of 15, the first issued
        // token must be 16 — never 1, which restore would present as stale.
        assert_eq!(
            first_token(&LocalLeaseProvider::with_floor(15), "rt").await,
            16
        );
        // floor 0 is equivalent to new().
        assert_eq!(
            first_token(&LocalLeaseProvider::with_floor(0), "rt").await,
            1
        );
    }
}
