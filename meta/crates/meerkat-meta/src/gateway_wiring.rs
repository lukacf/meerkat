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
//!
//! The identity session bridge ([`identity_session_bridge`]) is shared by
//! both binaries and by `UnifiedRuntimeBuilder`: three compositions, one
//! construction.

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

/// The identity-first session bridge a composition installs: Meta's bridge
/// over `handle`, with every seam `mob_runtime` recorded at its bootstrap.
/// `UnifiedRuntimeBuilder` and both gateway binaries build their identity
/// bridge here, so none of them can leave a seam out. The builder once built
/// its bridge without the controller pin source, and every governed send on
/// its runtime was refused after Meta had woken and built the member.
#[must_use]
pub fn identity_session_bridge(
    mob_runtime: &crate::mob_handle_runtime::MobRuntime,
    handle: meerkat_mob::MobHandle,
) -> crate::identity_first::MobSessionBridge {
    let mut bridge = match mob_runtime.session_service().cloned() {
        Some(service) => {
            crate::identity_first::MobSessionBridge::with_session_service(handle, service)
        }
        None => crate::identity_first::MobSessionBridge::new(handle),
    };
    // Heal seam (2026-07-29 incident): the continuity repair supervisor asks
    // this recoverer to commit the durable head before declaring an identity
    // healed; without it, heal is a cosmetic entry reset that the next
    // materialization re-Breaks.
    if let Some(recoverer) = mob_runtime.committed_boundary_recoverer() {
        bridge = bridge.with_committed_boundary_recoverer(recoverer);
    }
    // Image deliveries are judged with the member builder's host config, so a
    // self-hosted model's `vision` flag counts.
    if let Some(config) = mob_runtime.member_model_config() {
        bridge = bridge.with_member_model_config(config);
    }
    // Governed binding pins the member session actor's actual controller
    // through the concrete session owner, and a live MCP attachment reaches
    // the current member through it.
    if let Some(source) = mob_runtime.controller_pin_source() {
        bridge = bridge.with_controller_pin_source(source);
    }
    bridge
}
