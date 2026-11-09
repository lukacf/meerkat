//! Identities created at runtime: Core's created entries, whose records are
//! their rows in the mob's identity store (store-owned identities), not the
//! host's roster. An identity a host adopted into store custody is one too.
//!
//! This runtime serves them as it serves the identities its host's roster
//! declares, with the spec each one's row holds
//! ([`SessionBridge::created_identities`]):
//!
//! - each roster pass registers every created entry Core holds that the
//!   host's roster does not list, and forgets the registration of one Core
//!   no longer holds as a created entry (removed since);
//! - a door that names a created entry this runtime has not registered yet
//!   (one created since the last roster pass) registers it from its row
//!   first ([`IdentityRuntime::admit_unregistered_name`]), so a send by name,
//!   a dispatch or a schedule fire reaches its member through the identity's
//!   door.
//!
//! Nothing declares a created entry: Core's declared apply notes it
//! (`DeclaredEntryNote::OwnedByStore`) and changes nothing, and no complete
//! roster unlists it. Core's MobMachine drives its lifecycle (Core answers its
//! identity management `CreatedEntry`), so Meta's lifecycle verbs refuse it
//! ([`IdentityRuntimeError::IdentityStoreOwned`]), and the managed topology
//! never unwires an edge of it: its row's owned edges and the definition's
//! wiring are Core's.
//!
//! [`SessionBridge::created_identities`]: super::super::bridge::SessionBridge::created_identities

use std::collections::BTreeSet;

use tokio::sync::broadcast;

use super::super::bridge::BridgeError;
use super::super::types::{AgentIdentity, DurableAgentSpec};
use super::{
    IDENTITY_EVENT_CHANNEL_CAPACITY, IdentityEntry, IdentityRuntime, IdentityRuntimeError,
};

impl IdentityRuntime {
    /// The roster pass's step for the identities created at runtime: each
    /// created entry Core holds that `roster` does not list is registered
    /// with the spec its row holds, under its lifecycle lock, and the
    /// registration of a created identity Core no longer holds is forgotten.
    /// A read of the created entries that fails registers and forgets nothing
    /// (the next pass reads them again, and a door registers one on demand);
    /// over a bridge that cannot read them nothing is registered.
    pub(super) async fn register_created_identities(&self, roster: &[DurableAgentSpec]) {
        let Some(bridge) = self.bridge.clone() else {
            return;
        };
        let created = match bridge.created_identities().await {
            Ok(created) => created,
            Err(BridgeError::Unsupported { .. }) => return,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the roster pass could not read the identities created at runtime; it \
                     registers none this time, and a door registers one on demand"
                );
                return;
            }
        };
        let listed = roster
            .iter()
            .map(|spec| &spec.identity)
            .collect::<BTreeSet<_>>();
        let held = created
            .iter()
            .map(|spec| spec.identity.clone())
            .collect::<BTreeSet<_>>();
        for spec in created {
            // The roster's own listing of a created entry (an identity a host
            // adopted into store custody that its roster still lists) keeps
            // the roster's registration.
            if listed.contains(&spec.identity) {
                continue;
            }
            let lifecycle_lock = self.lifecycle_lock_for(&spec.identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            self.register_created_locked(spec).await;
        }
        let stale = self
            .entries
            .read()
            .await
            .iter()
            .filter(|(identity, entry)| {
                entry.created && !held.contains(*identity) && !listed.contains(identity)
            })
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        for identity in stale {
            self.forget_created_registration(&identity).await;
        }
    }

    /// Whether a door to `name` is the door of an identity this runtime
    /// serves: `Ok(true)` when `name` is registered, or when Core holds it as
    /// a created entry, which is registered now from its row; `Ok(false)`
    /// when Core holds the name for no identity (it is a member of the mob's
    /// raw plane, if anything), and without a session bridge; and the door's
    /// typed refusal of a name Core holds for an identity this runtime does
    /// not serve ([`IdentityRuntime::unregistered_entry_refusal`]). A read
    /// Core could not serve refuses, retryable, never read as "no
    /// identity's".
    pub(crate) async fn admit_unregistered_name(
        &self,
        name: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        if self.contains(name).await {
            return Ok(true);
        }
        let Some(entry) = self.core_entry_of_name(name).await? else {
            return Ok(false);
        };
        if entry.is_created_entry() {
            return self.register_created_entry(name).await;
        }
        match self.unregistered_entry_refusal(name, &entry).await {
            Some(refusal) => Err(refusal),
            None => Ok(false),
        }
    }

    /// Register `name`, which Core holds as a created entry, from its row in
    /// the mob's identity store, under its lifecycle lock; a registration a
    /// roster pass or another door made meanwhile stands. A name whose row
    /// Core no longer holds (removed meanwhile) or whose row carries no spec,
    /// and a name over a bridge that cannot read created identities, is
    /// unknown to this runtime; a read that failed refuses, retryable.
    async fn register_created_entry(
        &self,
        name: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        let bridge = self.core_bridge(name)?;
        let spec = match bridge.created_identity(name).await {
            Ok(Some(spec)) => spec,
            Ok(None) | Err(BridgeError::Unsupported { .. }) => {
                return Err(IdentityRuntimeError::UnknownIdentity(name.clone()));
            }
            Err(error) => {
                return Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                    identity: name.clone(),
                    detail: format!(
                        "reading the row of the identity created at runtime in the mob's \
                         identity store: {error}"
                    ),
                });
            }
        };
        let lifecycle_lock = self.lifecycle_lock_for(name).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if !self.contains(name).await {
            self.register_created_locked(spec).await;
        }
        Ok(true)
    }

    /// Record `spec`, a created entry's row, as its identity's registration,
    /// marked created, while the caller holds its lifecycle lock. A wake
    /// refusal recorded for it survives, as for any re-registration.
    pub(super) async fn register_created_locked(&self, spec: DurableAgentSpec) {
        let identity = spec.identity.clone();
        let bootstrap_generation = self.identity_bootstrap_status_with_generation().0;
        {
            let mut entries = self.entries.write().await;
            let wake_refusal = entries
                .get(&identity)
                .and_then(|existing| existing.wake_refusal.clone());
            entries.insert(
                identity.clone(),
                IdentityEntry {
                    spec,
                    bootstrap_generation,
                    wake_refusal,
                    last_delivery_error: None,
                    last_reload: None,
                    declaration_refused: false,
                    created: true,
                },
            );
        }
        // The event channel outlives a re-registration: its subscribers
        // belong to the identity's life, not to its row.
        self.event_channels
            .write()
            .await
            .entry(identity)
            .or_insert_with(|| broadcast::channel(IDENTITY_EVENT_CHANNEL_CAPACITY).0);
    }

    /// Forget the registration of `identity`, an identity created at runtime
    /// that Core no longer held as a created entry when the roster pass read
    /// them, under its lifecycle lock: only while its registration is still a
    /// created one and Core's entry of its name, read again under the lock,
    /// is no created entry (one a door registered after the pass's read
    /// stays). A read that fails keeps the registration for a later pass.
    async fn forget_created_registration(&self, identity: &AgentIdentity) {
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        let created = self
            .entries
            .read()
            .await
            .get(identity)
            .is_some_and(|entry| entry.created);
        if !created {
            return;
        }
        match self.core_entry_of_name(identity).await {
            Ok(Some(entry)) if entry.is_created_entry() => return,
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(
                    %identity,
                    %error,
                    "Core's entry of a created identity's name did not read; its registration \
                     stays until a later roster pass"
                );
                return;
            }
        }
        self.event_channels.write().await.remove(identity);
        self.entries.write().await.remove(identity);
    }

    /// The identities this runtime registered as created at runtime: their
    /// edges are their rows' owned edges and the definition's wiring, Core's
    /// both, never the managed topology's to unwire.
    pub(crate) async fn created_registrations(&self) -> BTreeSet<AgentIdentity> {
        self.entries
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.created)
            .map(|(identity, _)| identity.clone())
            .collect()
    }
}
