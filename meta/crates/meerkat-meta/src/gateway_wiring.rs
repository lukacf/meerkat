//! Shared gateway construction seams (doctrine D2: converge construction,
//! merge binaries later).
//!
//! The K1/K2 field failures came from the two gateway binaries embedding the
//! same `UnifiedRuntime` with DIVERGENT wiring — `rpc_gateway` built the full
//! identity substrate, `meta_gateway` built none. Every piece both binaries
//! must construct identically belongs here so the drift class is structural
//! history. The full store construction converges with the 0.8 binary merge;
//! v1 shares the identity substrate, whose divergence caused the field
//! failures.

use std::path::Path;
use std::sync::Arc;

use crate::identity_first::{
    ContinuityStore, LocalContinuityStore, LocalLeaseProvider, contracts::LeaseProvider,
};

/// The durable-identity substrate a gateway builds from its state directory:
/// the continuity store and a lease provider whose fencing counter resumes
/// ABOVE the persisted high-water, so a restart with existing continuity
/// history never presents a stale token.
pub struct GatewayIdentitySubstrate {
    pub continuity_store: Arc<dyn ContinuityStore>,
    pub lease_provider: Arc<dyn LeaseProvider>,
}

/// Open the local identity substrate at `continuity_db`.
///
/// Fails loudly rather than degrading: a 0 fencing floor on an existing
/// store would re-arm the restart-abort class the floor seeding prevents.
pub async fn open_identity_substrate(
    continuity_db: &Path,
) -> Result<GatewayIdentitySubstrate, String> {
    let (store, fencing_floor) =
        LocalContinuityStore::open_with_fencing_floor(continuity_db.to_path_buf())
            .await
            .map_err(|e| {
                format!(
                    "failed to open continuity store and read fencing high-water at {}: {e}",
                    continuity_db.display()
                )
            })?;
    Ok(GatewayIdentitySubstrate {
        continuity_store: Arc::new(store),
        lease_provider: Arc::new(LocalLeaseProvider::with_floor(fencing_floor)),
    })
}

/// The mob handle a gateway gives the identity runtime it installs, for its
/// services and session bridge. The installed identity runtime lives inside
/// the composition the public owner keeps alive, so this is a back-edge and
/// does not own the mob (D67), as in the builder: an owning handle there
/// would keep admission open after the last external owner drops, and the
/// runtime's retained teardown would wait on that close indefinitely.
#[must_use]
pub fn identity_mob_handle(runtime: &crate::UnifiedRuntime) -> meerkat_mob::MobHandle {
    runtime.mob_handle().observer()
}
