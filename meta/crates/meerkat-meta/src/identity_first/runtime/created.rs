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
//!   first ([`IdentityRuntime::admit_unregistered_name`]): the send,
//!   dispatch and governed doors, the member mutations, a schedule fire,
//!   and the reads of its status, health, events and delivery scope;
//! - a delivery door (a send, a dispatch, a governed input) reads the row of
//!   a created registration again ([`IdentityRuntime::admit_at_delivery_door`]),
//!   so the addressability, labels, profile and context it serves are the
//!   row's current ones, and forgets the registration of one Core no longer
//!   holds;
//! - a registration keeps the identity's customizer tool dispatcher when the
//!   host composes an agent customizer, so each build of its member that Core
//!   starts afterwards carries the host customizer's tools, as a roster
//!   identity's does;
//! - Meta's member build preparer registers a created entry's member from
//!   its row when Core builds it before a pass or a door registered it
//!   ([`IdentityRuntime::register_created_member_for_build`]): the build is
//!   recorded as the identity's, and the member's next build carries the
//!   tools.
//!
//! A delivery to a created identity Core holds no member of is answered
//! from Core's entry ([`IdentityRuntime::unseated_created_refusal`]).
//!
//! Nothing declares a created entry: Core's declared apply notes it
//! (`DeclaredEntryNote::OwnedByStore`) and changes nothing, and no complete
//! roster unlists it. Core's MobMachine drives its lifecycle (Core answers its
//! identity management `CreatedEntry`), so Meta's lifecycle verbs refuse it
//! ([`IdentityRuntimeError::IdentityStoreOwned`]), and the managed topology
//! never prunes a live edge of it that this runtime's topology did not wire:
//! its row's owned edges and the definition's wiring are Core's.
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
    /// identity's". It takes no lifecycle lock, so a caller holding one may
    /// ask.
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

    /// [`Self::admit_unregistered_name`] for a delivery door (a send, a
    /// dispatch, a governed input), which delivers under the registered row:
    /// the registration of an identity created at runtime is read again from
    /// its row (one store read, no actor command). A row Core changed since
    /// (an update of its addressability, labels, profile or context) is
    /// registered now, and the registration of one Core no longer holds as a
    /// created entry (removed since) is forgotten, the name then answered as
    /// one this runtime has not registered. A read Core could not serve
    /// refuses, retryable. The caller holds no lifecycle lock: a changed row
    /// is registered under the identity's.
    pub(crate) async fn admit_at_delivery_door(
        &self,
        name: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        let created = self
            .entries
            .read()
            .await
            .get(name)
            .map(|entry| entry.created);
        match created {
            Some(true) => self.refresh_created_registration(name).await,
            Some(false) => Ok(true),
            None => self.admit_unregistered_name(name).await,
        }
    }

    /// Read the row of `name`, registered as created at runtime, again: see
    /// [`Self::admit_at_delivery_door`].
    async fn refresh_created_registration(
        &self,
        name: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        let bridge = self.core_bridge(name)?;
        match bridge.created_identity(name).await {
            Ok(Some(spec)) => {
                let current = self
                    .entries
                    .read()
                    .await
                    .get(name)
                    .map(|entry| (entry.created, entry.spec == spec));
                // Registered from this very row, or by the host's roster
                // meanwhile: nothing to change.
                if matches!(current, Some((true, true) | (false, _))) {
                    return Ok(true);
                }
                let lifecycle_lock = self.lifecycle_lock_for(name).await;
                let _lifecycle_guard = lifecycle_lock.lock().await;
                // A registration the host's roster made meanwhile stands.
                let roster_registered = self
                    .entries
                    .read()
                    .await
                    .get(name)
                    .is_some_and(|entry| !entry.created);
                if !roster_registered {
                    self.register_created_locked(spec).await;
                }
                Ok(true)
            }
            Ok(None) => {
                self.forget_created_registration(name).await;
                if self.contains(name).await {
                    return Ok(true);
                }
                let Some(entry) = self.core_entry_of_name(name).await? else {
                    return Ok(false);
                };
                if entry.is_created_entry() {
                    // A created entry whose row carries no spec this build
                    // reads is no identity this runtime serves.
                    return Err(IdentityRuntimeError::UnknownIdentity(name.clone()));
                }
                match self.unregistered_entry_refusal(name, &entry).await {
                    Some(refusal) => Err(refusal),
                    None => Ok(false),
                }
            }
            Err(BridgeError::Unsupported { .. }) => Ok(true),
            Err(error) => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: name.clone(),
                detail: format!(
                    "reading the row of the identity created at runtime in the mob's identity \
                     store: {error}"
                ),
            }),
        }
    }

    /// Register `name`, which Core holds as a created entry, from its row in
    /// the mob's identity store; a registration a roster pass or another
    /// door made meanwhile stands. A name whose row Core no longer holds
    /// (removed meanwhile) or whose row carries no spec, and a name over a
    /// bridge that cannot read created identities, is unknown to this
    /// runtime; a read that failed refuses, retryable. It takes no lifecycle
    /// lock: the registration is one step under the registration table's
    /// write lock.
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
        self.register_created_if_absent(spec).await;
        Ok(true)
    }

    /// Register the member Core is about to build, `identity`'s, when Core
    /// holds `identity` as a created entry: from its row, as a door would
    /// ([`Self::admit_unregistered_name`]), so Meta's build preparer prepares
    /// it as a registered identity's (the host's customizer runs, and the
    /// build is recorded as the identity's). The build Core started before
    /// the registration carries no customizer tools: Core asked Meta's spawn
    /// customizer for its tool dispatcher first. `Ok(false)` when Core holds
    /// no created entry of it, its row carries no spec, or the bridge cannot
    /// read created identities. It runs inside Core's build of the member,
    /// so it never waits on the identity's lifecycle lock (a lifecycle
    /// operation holding it may be the one Core builds for): the
    /// registration is one step under the registration table's write lock,
    /// and one already made stands.
    pub(crate) async fn register_created_member_for_build(
        &self,
        identity: &AgentIdentity,
    ) -> Result<bool, String> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(false);
        };
        match bridge.created_identity(identity).await {
            Ok(Some(spec)) => {
                self.register_created_if_absent(spec).await;
                Ok(true)
            }
            Ok(None) | Err(BridgeError::Unsupported { .. }) => Ok(false),
            Err(error) => Err(format!(
                "the row of the identity created at runtime could not be read: {error}"
            )),
        }
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
                Self::created_entry(spec, bootstrap_generation, wake_refusal),
            );
        }
        self.ensure_registration_channels(identity).await;
    }

    /// Record `spec`, a created entry's row, as its identity's registration,
    /// marked created, unless the identity is registered already: the check
    /// and the record are one step under the registration table's write
    /// lock, so a registration made meanwhile (a roster pass's, a door's)
    /// stands. No lifecycle lock is taken or needed: nothing is replaced.
    async fn register_created_if_absent(&self, spec: DurableAgentSpec) {
        let identity = spec.identity.clone();
        let bootstrap_generation = self.identity_bootstrap_status_with_generation().0;
        self.entries
            .write()
            .await
            .entry(identity.clone())
            .or_insert_with(|| Self::created_entry(spec, bootstrap_generation, None));
        self.ensure_registration_channels(identity).await;
    }

    /// A created entry's registration of `spec`.
    fn created_entry(
        spec: DurableAgentSpec,
        bootstrap_generation: u64,
        wake_refusal: Option<super::super::types::WakeRefusal>,
    ) -> IdentityEntry {
        IdentityEntry {
            spec,
            bootstrap_generation,
            wake_refusal,
            last_delivery_error: None,
            last_reload: None,
            declaration_refused: false,
            created: true,
        }
    }

    /// What a registration of `identity` keeps beside its row: its event
    /// channel, kept across re-registrations (its subscribers belong to the
    /// identity's life, not to its row), and, when the host composes an
    /// agent customizer, its stable customizer tool dispatcher. Meta's spawn
    /// customizer attaches that dispatcher to every build of the identity's
    /// member from now on, as it does a roster identity's (whose member
    /// carries the identity label), and the build preparer publishes the
    /// host customizer's tools into it.
    async fn ensure_registration_channels(&self, identity: AgentIdentity) {
        if let Some(registry) = self.customizer_tools.read().await.clone() {
            registry.ensure(&identity);
        }
        self.event_channels
            .write()
            .await
            .entry(identity)
            .or_insert_with(|| broadcast::channel(IDENTITY_EVENT_CHANNEL_CAPACITY).0);
    }

    /// Forget the registration of `identity`, an identity created at runtime
    /// that Core no longer held as a created entry when the roster pass or a
    /// delivery door read it, under its lifecycle lock: only while its
    /// registration is still a created one and Core's entry of its name,
    /// read again under the lock, is no created entry (one a door registered
    /// after the pass's read stays). A read that fails keeps the
    /// registration for a later pass.
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

    /// The refusal of a delivery to `identity`, registered as created at
    /// runtime, when Core holds no member of it, answered from Core's entry
    /// of its name: archived, a final refusal naming the archive
    /// ([`IdentityRuntimeError::IdentityArchived`]: Core's unarchive seats it
    /// again); awaiting its host's preparation, its held creation or its
    /// seat, retryable ([`IdentityRuntimeError::IdentityWakeRetryable`]:
    /// Core seats it once its host settles the preparation, its host's held
    /// protocol creates it, or the mob's next start seats it). `None` for
    /// any other entry (an identity its host creates on its first use
    /// included), which the caller answers with its view state; a read Core
    /// could not serve refuses, retryable.
    pub(super) async fn unseated_created_refusal(
        &self,
        identity: &AgentIdentity,
    ) -> Option<IdentityRuntimeError> {
        let entry = match self.core_entry_of_name(identity).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return None,
            Err(error) => return Some(error),
        };
        if !entry.is_created_entry() || entry.member.is_some() {
            return None;
        }
        let detail = match entry.unseated? {
            meerkat_mob::UnseatedEntryCause::Archived => {
                return Some(IdentityRuntimeError::IdentityArchived {
                    identity: identity.clone(),
                });
            }
            meerkat_mob::UnseatedEntryCause::PreparationPending => {
                "the identity created at runtime awaits its host's preparation: Core seats its \
                 member once the host settles it. Nothing was delivered; deliver again then"
            }
            meerkat_mob::UnseatedEntryCause::HeldCreation => {
                "the identity created at runtime awaits its held creation by its host. Nothing \
                 was delivered; deliver again once its host created it"
            }
            meerkat_mob::UnseatedEntryCause::SeatOwed => {
                "Core owes the identity created at runtime its seat, which the mob's next start \
                 seats. Nothing was delivered; deliver again once Core seated it"
            }
            _ => return None,
        };
        Some(IdentityRuntimeError::IdentityWakeRetryable {
            identity: identity.clone(),
            session_id: None,
            detail: detail.to_string(),
        })
    }

    /// The identities this runtime registered as created at runtime: a live
    /// edge of one that this runtime's topology did not wire is its row's
    /// owned edge or the definition's wiring, Core's both, never the managed
    /// topology's to prune.
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
