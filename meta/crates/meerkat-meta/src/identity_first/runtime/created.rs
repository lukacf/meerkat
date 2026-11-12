//! Where an identity's record lives, and the only places that depends on it.
//!
//! An identity is one entry of Core's mob, whoever asked for it. Its spec
//! lives in one of two records: the host's roster, which the host declares
//! again at each pass, or its row in the mob's identity store (an identity
//! created at run time, or one a host adopted into the store). Every verb,
//! read, grant, wake and delivery of this runtime serves both alike. Which
//! record holds the spec decides three things only, and this module is the
//! one place that asks ([`IdentityRuntime::spec_record`], from Core's typed
//! identity management):
//!
//! - **which record the spec is read from**: a roster pass registers the
//!   identities the roster lists, and this module registers the others from
//!   their rows ([`IdentityRuntime::register_created_identities`], a door
//!   that names one first, the member build preparer, and
//!   [`IdentityRuntime::register_from_row`] for a change that just
//!   committed); a delivery door and a reset read the row again
//!   ([`IdentityRuntime::admit_at_delivery_door`],
//!   [`IdentityRuntime::adopt_current_spec_locked`]);
//! - **how a seat is requested**: an identity the roster holds is seated
//!   again by its declaration, so its member is retired and declared again;
//!   an identity its row holds is never declared, so its successor is Core's
//!   own same-session step
//!   ([`IdentityRuntime::end_incarnation_for_successor_locked`]);
//! - **which names a roster pass acts on**: only the names the roster
//!   declares ([`IdentityRuntime::roster_pass_acts_on`],
//!   [`IdentityRuntime::roster_held`]).
//!
//! The roster half of each goes away once a host's configuration is declared
//! into the identity store: every identity then has a row, and the row's
//! half is the one that stays.
//!
//! The registration of a row's identity keeps the identity's customizer tool
//! dispatcher when the host composes an agent customizer, as a roster
//! identity's does, and Meta's spawn customizer attaches it to every build
//! of a member Core holds as an identity entry, so the first build of a new
//! identity carries the host customizer's tools.
//!
//! A delivery to an identity Core holds no member of is answered from Core's
//! entry of its name ([`IdentityRuntime::unseated_refusal`]): archived,
//! final; awaiting its host's preparation, its held creation or its seat,
//! retryable.
//!
//! [`SessionBridge::created_identity`]: super::super::bridge::SessionBridge::created_identity

use std::collections::BTreeSet;

use tokio::sync::broadcast;

use super::super::bridge::{BridgeError, MemberRuntimeBinding};
use super::super::first_boot_manifest::{FirstBootRow, FirstBootRowState};
use super::super::types::{AgentIdentity, DurableAgentSpec};
use super::{
    IDENTITY_EVENT_CHANNEL_CAPACITY, IdentityEntry, IdentityRuntime, IdentityRuntimeError,
};

/// Which record holds an identity's spec (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecRecord {
    /// The host's roster, declared again at each pass.
    Roster,
    /// The identity's row in the mob's identity store.
    Store,
}

/// Why a member's incarnation is ended for a successor on its own session
/// ([`IdentityRuntime::end_incarnation_for_successor_locked`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SuccessorCause {
    /// The respawn verb.
    Respawn,
    /// A reload over a bridge that has no registration reload.
    Reload,
    /// A maintenance rebuild that changes the member's compaction floor to
    /// the one it names (`None` removes it): the roster's declaration lowers
    /// it into the build it seats, and Core's same-session successor is
    /// seated with it.
    CompactionFloor(Option<std::num::NonZeroU64>),
}

/// Whether `binding` is the member of an identity whose record is its row in
/// the mob's identity store, by Core's typed identity management of its name
/// as the binding's read answered it. Under such a name Core seats the
/// identity's own member only, so the member is the identity's with no mark
/// of an identity-first spawn
/// ([`super::binding_spawned_for_identity`]).
pub(super) fn binding_is_a_rows(binding: &meerkat_mob::MemberBinding) -> bool {
    binding.identity_management() == meerkat_mob::MemberIdentityManagement::CreatedEntry
}

impl IdentityRuntime {
    /// Which record holds `identity`'s spec, from Core's typed identity
    /// management of its name: the host's roster, or its row in the mob's
    /// identity store. An identity a host adopted through an identity intent
    /// row (Core's identity reconciler drives it) is refused, typed, as it
    /// always was: no verb of this runtime acts on it. A row this build
    /// cannot read, or a management kind it does not know, refuses too
    /// (history unavailable): nothing acts on an identity this runtime could
    /// not prove is its own. Without a session bridge there is no Core mob,
    /// and the roster holds every identity.
    pub(crate) async fn spec_record(
        &self,
        identity: &AgentIdentity,
    ) -> Result<SpecRecord, IdentityRuntimeError> {
        if self.bridge.is_none() {
            return Ok(SpecRecord::Roster);
        }
        Self::spec_record_of(self.core_identity_management(identity).await?, identity)
    }

    /// [`Self::spec_record`] of one management answer.
    pub(super) fn spec_record_of(
        management: meerkat_mob::MemberIdentityManagement,
        identity: &AgentIdentity,
    ) -> Result<SpecRecord, IdentityRuntimeError> {
        match management {
            meerkat_mob::MemberIdentityManagement::CommandDriven => Ok(SpecRecord::Roster),
            meerkat_mob::MemberIdentityManagement::CreatedEntry => Ok(SpecRecord::Store),
            meerkat_mob::MemberIdentityManagement::IntentManaged => {
                Err(IdentityRuntimeError::IdentityHostAdopted {
                    identity: identity.clone(),
                })
            }
            // An intent row this build cannot read may declare anything:
            // never act on it until it reads.
            meerkat_mob::MemberIdentityManagement::IntentUnreadable => {
                Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                    identity: identity.clone(),
                    detail: "Core holds an identity intent row for the identity that this \
                             build cannot read"
                        .to_string(),
                })
            }
            // A management kind this build does not know: never act on it.
            _ => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: "Core reports an identity management kind this build does not know"
                    .to_string(),
            }),
        }
    }

    /// Whether a roster pass may change `identity`'s Core member: only an
    /// identity the roster holds. A pass never retires, moves or unlists an
    /// identity whose record is its row, nor one a host adopted, nor one
    /// whose management Core cannot read.
    pub(super) async fn roster_pass_acts_on(
        &self,
        identity: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        if self.bridge.is_none() {
            return Ok(false);
        }
        Ok(matches!(
            self.spec_record(identity).await,
            Ok(SpecRecord::Roster)
        ))
    }

    /// Whether an identity holds `name` through its row in the mob's
    /// identity store, seated or not: a roster add of the name adds nothing.
    /// A removed identity's name is no such entry, and Core answers it
    /// itself. Without a session bridge, and over one that cannot read
    /// Core's entries (it serves no identity that has a row), no row holds a
    /// name.
    pub(super) async fn row_holds_name(
        &self,
        name: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(false);
        };
        match bridge.identity_entry(name).await {
            Ok(entry) => Ok(entry.is_created_entry()),
            Err(BridgeError::Unsupported { .. }) => Ok(false),
            Err(error) => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: name.clone(),
                detail: format!("reading Core's entry of the name: {error}"),
            }),
        }
    }

    /// Of `names`, the identities the roster holds: the names a roster pass
    /// acts on. One read of Core's member history serves them all; when it
    /// does not read, every name is answered, and Core decides each (its
    /// declared apply never unlists an identity whose record is its row).
    pub(super) async fn roster_held(&self, names: Vec<AgentIdentity>) -> Vec<AgentIdentity> {
        if names.is_empty() {
            return names;
        }
        let Ok(Some(history)) = self.core_view_history().await else {
            return names;
        };
        names
            .into_iter()
            .filter(|identity| {
                history.identity_management(&crate::member_comms_id::roster_member_id_for_identity(
                    identity.as_str(),
                )) == meerkat_mob::MemberIdentityManagement::CommandDriven
            })
            .collect()
    }

    /// Read `identity`'s spec again from the record that holds it, before a
    /// fresh life is seated with it, while the caller holds the identity's
    /// lifecycle lock: the host's roster (through the roster provider the
    /// runtime was given), or the identity's row.
    pub(super) async fn adopt_current_spec_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        match self.spec_record(identity).await? {
            SpecRecord::Roster => self.adopt_current_roster_spec_for_reset(identity).await,
            SpecRecord::Store => self.adopt_row_spec_locked(identity).await.map(|_| ()),
        }
    }

    /// Register `identity` again from its row, while the caller holds its
    /// lifecycle lock, and answer the row's spec; the registered spec when
    /// the bridge cannot read rows. A row Core no longer holds refuses: the
    /// identity was removed meanwhile.
    async fn adopt_row_spec_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<DurableAgentSpec, IdentityRuntimeError> {
        let bridge = self.core_bridge(identity)?;
        match bridge.created_identity(identity).await {
            Ok(Some(spec)) => {
                self.register_created_locked(spec.clone()).await;
                Ok(spec)
            }
            Ok(None) => Err(IdentityRuntimeError::UnknownIdentity(identity.clone())),
            Err(BridgeError::Unsupported { .. }) => self
                .entries
                .read()
                .await
                .get(identity)
                .map(|entry| entry.spec.clone())
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone())),
            Err(error) => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading the identity's row in the mob's identity store: {error}"),
            }),
        }
    }

    /// End `identity`'s live incarnation `live` so that its successor runs
    /// on the same session at the next generation, while the caller holds
    /// the identity's lifecycle lock; the caller's wake then materializes the
    /// successor. The one place a respawn, a reload's fallback and a
    /// maintenance rebuild ask for a seat, by the record that holds the
    /// identity's spec:
    ///
    /// - the host's roster: Core retires the incarnation, and the wake's
    ///   declaration seats the identity again on its archived session with
    ///   the roster's current spec. This half goes away once a host's
    ///   configuration is declared into the identity store;
    /// - the identity's row: no declaration seats such an identity, so Core
    ///   replaces the incarnation by its successor in one step
    ///   ([`super::super::bridge::SessionBridge::respawn_member_on_own_session`]),
    ///   seated with the row's current profile, runtime mode and labels, and
    ///   with the compaction floor a maintenance rebuild names; the identity
    ///   is never without a member.
    ///
    /// Either way what the retirement owes is recorded before Core is asked,
    /// as a reload's: a successor on the same session owes no side effect.
    /// `step` names the caller's step in a failure. `Ok(true)` when Core
    /// seated the successor already (the row's identity): the caller hands
    /// that to its wake (`EmbodimentOverrides::successor_seated`), which
    /// announces the successor as the wake that seats a roster identity's
    /// does.
    pub(super) async fn end_incarnation_for_successor_locked(
        &self,
        identity: &AgentIdentity,
        live: &MemberRuntimeBinding,
        cause: SuccessorCause,
        step: &'static str,
    ) -> Result<bool, IdentityRuntimeError> {
        let record = self.spec_record(identity).await?;
        let row_spec = match record {
            SpecRecord::Roster => None,
            SpecRecord::Store => Some(self.adopt_row_spec_locked(identity).await?),
        };
        let prepared = self
            .prepare_lifecycle_retirement(
                identity,
                crate::runtime::LifecycleEffectVerb::Reload,
                None,
            )
            .await?;
        let bridge = self.core_bridge(identity)?;
        let ended = match row_spec.as_ref() {
            None => bridge.retire_member(&live.agent_runtime_id).await,
            Some(spec) => {
                // The compaction floor change the successor's seat carries,
                // when the cause is one.
                let floor_change = match cause {
                    SuccessorCause::CompactionFloor(floor) => Some(floor),
                    SuccessorCause::Respawn | SuccessorCause::Reload => None,
                };
                bridge
                    .respawn_member_on_own_session(identity, spec, floor_change)
                    .await
            }
        };
        if let Err(error) = ended {
            // A step Core refused before any of it started retired nothing:
            // what was recorded for it is taken back, and the refusal is
            // retryable.
            if let BridgeError::RotationRefused { detail, .. } = &error {
                self.abandon_lifecycle_retirement(prepared).await;
                return Err(IdentityRuntimeError::IdentityWakeRetryable {
                    identity: identity.clone(),
                    session_id: live.session_id.clone(),
                    detail: format!(
                        "Core refused the {step} before any of it started, and nothing was \
                         retired; ask again once the member's other lifecycle operation has \
                         ended ({detail})"
                    ),
                });
            }
            return Err(IdentityRuntimeError::Internal(format!(
                "bridge {step}: {error}"
            )));
        }
        self.settle_lifecycle_retirement(prepared).await;
        // Seated already (the row's identity), or seated by the caller's
        // wake (the roster's): the caller tells its wake which.
        Ok(row_spec.is_some())
    }

    /// The roster pass's step for the identities whose record is their row:
    /// each one Core holds that `roster` does not list and this runtime has
    /// not registered from its row yet is registered now, with the spec its
    /// row holds, and the registration of one Core no longer holds is
    /// forgotten. The names come from Core's identity entry pages (the
    /// machine state its actor published: no store read), and a row is read
    /// only for a name not registered yet, so a pass over identities this
    /// runtime serves already reads no row; a row Core changed since is read
    /// again at the identity's delivery door, or when its change says so
    /// ([`Self::register_from_row`]). A read that fails registers and
    /// forgets nothing (the next pass reads again, and a door registers one
    /// on demand); over a bridge that cannot read them nothing is
    /// registered.
    ///
    /// Answers the specs of the identities this runtime serves from their
    /// rows: the pass tracks them beside the roster's. A row is its
    /// identity's one owner, also while the host's roster still lists its
    /// name (a host adopted it into the store): such a name is registered
    /// from its row, replacing a registration the roster made.
    pub(super) async fn register_created_identities(&self) -> Vec<DurableAgentSpec> {
        if let Some(bridge) = self.bridge.clone() {
            match bridge.created_identity_names().await {
                Ok(names) => {
                    let held = names.iter().cloned().collect::<BTreeSet<_>>();
                    // The names not registered from their rows yet, each
                    // with whether the roster registered it.
                    let unregistered = {
                        let entries = self.entries.read().await;
                        names
                            .into_iter()
                            .filter_map(|name| {
                                match entries.get(&name).map(|entry| entry.created) {
                                    Some(true) => None,
                                    registered => Some((name, registered.is_some())),
                                }
                            })
                            .collect::<Vec<_>>()
                    };
                    if !unregistered.is_empty() {
                        let asked = unregistered
                            .iter()
                            .map(|(name, _)| name.clone())
                            .collect::<Vec<_>>();
                        // Their rows in one read, in pages: a cold start
                        // makes no store read per identity.
                        match bridge.created_identities_of(&asked).await {
                            Ok(mut rows) => {
                                for (name, registered_by_roster) in unregistered {
                                    let Some(spec) = rows.remove(&name) else {
                                        continue;
                                    };
                                    if registered_by_roster {
                                        // The roster's registration of a
                                        // name a row holds gives way to the
                                        // row's.
                                        let lifecycle_lock = self.lifecycle_lock_for(&name).await;
                                        let _lifecycle_guard = lifecycle_lock.lock().await;
                                        self.register_created_locked(spec).await;
                                    } else {
                                        // A name not registered yet waits on
                                        // no lock: nothing is replaced.
                                        self.register_created_if_absent(spec).await;
                                    }
                                }
                            }
                            Err(BridgeError::Unsupported { .. }) => {}
                            Err(error) => tracing::warn!(
                                %error,
                                "the roster pass could not read the rows of the identities it \
                                 has not registered yet; the next pass, or a door that names \
                                 one, registers it"
                            ),
                        }
                    }
                    let stale = self
                        .entries
                        .read()
                        .await
                        .iter()
                        .filter(|(identity, entry)| entry.created && !held.contains(*identity))
                        .map(|(identity, _)| identity.clone())
                        .collect::<Vec<_>>();
                    for identity in stale {
                        self.forget_created_registration(&identity).await;
                    }
                }
                Err(BridgeError::Unsupported { .. }) => {}
                Err(error) => tracing::warn!(
                    %error,
                    "the roster pass could not read the identities whose record is their row; \
                     it registers none this time, and a door registers one on demand"
                ),
            }
        }
        self.row_registered_specs().await
    }

    /// The specs of the identities this runtime serves from their rows, as
    /// registered: no read of Core or of a row.
    pub(super) async fn row_registered_specs(&self) -> Vec<DurableAgentSpec> {
        self.entries
            .read()
            .await
            .values()
            .filter(|entry| entry.created)
            .map(|entry| entry.spec.clone())
            .collect()
    }

    /// `roster` without the identities this runtime serves from their rows:
    /// a row is its identity's one owner, also while the host's roster
    /// still lists its name, so a pass neither registers nor declares the
    /// roster's listing of it. No read of Core or of a row.
    pub(super) async fn roster_without_row_identities(
        &self,
        roster: &[DurableAgentSpec],
    ) -> Vec<DurableAgentSpec> {
        let entries = self.entries.read().await;
        roster
            .iter()
            .filter(|spec| {
                !entries
                    .get(&spec.identity)
                    .is_some_and(|entry| entry.created)
            })
            .cloned()
            .collect()
    }

    /// [`Self::register_created_identities`], answering the identities a
    /// roster pass tracks beside the roster's: every one it serves from its
    /// row but an archived one, which keeps no member until its unarchive
    /// (read from Core's entry of its name: the machine state its actor
    /// published).
    pub(super) async fn tracked_row_identities(&self) -> Vec<DurableAgentSpec> {
        let mut tracked = Vec::new();
        for spec in self.register_created_identities().await {
            let archived = matches!(
                self.core_entry_of_name(&spec.identity).await,
                Ok(Some(entry))
                    if entry.member.is_none()
                        && entry.unseated == Some(meerkat_mob::UnseatedEntryCause::Archived)
            );
            if !archived {
                tracked.push(spec);
            }
        }
        Box::pin(self.carry_out_fresh_rows(&tracked)).await;
        tracked
    }

    /// The roster pass's carry-out of the `Fresh` rows the offline identity
    /// reset left for the identities whose record is their row (`tracked`,
    /// registered a moment ago): the roster's declaration carries such a row
    /// out for an identity the roster lists, and no roster lists these. Each
    /// is carried out under its identity's lifecycle lock
    /// ([`Self::carry_out_fresh_row_locked`]); one that fails keeps its row
    /// for the next pass or the identity's next door, and is logged. The
    /// manifest is rewritten once, after the locks are released.
    async fn carry_out_fresh_rows(&self, tracked: &[DurableAgentSpec]) {
        if tracked.is_empty() || self.load_first_boot_manifest().await.is_err() {
            // An unreadable manifest refuses the roster's declaration, typed.
            return;
        }
        let rows = self.pending_first_boot_rows_snapshot();
        let mut settled = false;
        for spec in tracked {
            let identity = &spec.identity;
            if !rows.contains_key(identity) {
                continue;
            }
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            match self.carry_out_fresh_row_locked(identity).await {
                Ok(carried_out) => settled |= carried_out,
                Err(error) => tracing::warn!(
                    %identity,
                    %error,
                    "the fresh life an offline reset left owed was not started; its row stays \
                     for the next roster pass or the identity's next delivery"
                ),
            }
        }
        if settled {
            self.finish_declaration(&[], true).await;
        }
    }

    /// Carry out the `Fresh` row the offline identity reset left pending for
    /// `identity`, whose record is its row in the mob's identity store,
    /// while the caller holds the identity's lifecycle lock: the fresh life
    /// the running gateway's reset gives it. Core rotates its member onto a
    /// fresh session, seated from its row's current spec; nothing is built
    /// or woken, and its name and row stay. A row Core's history shows
    /// carried out already (the process ended before the manifest was
    /// rewritten) is settled. `Ok(true)` when the row is settled in memory:
    /// the caller rewrites the manifest. An identity the roster holds, a row
    /// of another kind, a row Core's history contradicts and a candidate
    /// launch are left to the roster's declaration, which answers them.
    pub(super) async fn carry_out_fresh_row_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        let Some(row) = self.pending_first_boot_rows_snapshot().remove(identity) else {
            return Ok(false);
        };
        if !matches!(row, FirstBootRow::Fresh { .. })
            || self.is_candidate_launch()
            || self.spec_record(identity).await? != SpecRecord::Store
        {
            return Ok(false);
        }
        let history = self
            .core_bridge(identity)?
            .member_binding_history()
            .await
            .map_err(|error| IdentityRuntimeError::MemberHistoryUnavailable {
                detail: format!(
                    "reading Core's member history to carry out a first-boot row: {error}"
                ),
            })?;
        match row.state_in(&history) {
            FirstBootRowState::Contradicted { .. } => return Ok(false),
            FirstBootRowState::Done => {}
            FirstBootRowState::ToDo => {
                self.adopt_current_spec_locked(identity).await?;
                let last = self.core_last_binding(identity).await?;
                // The fresh life's future is held behind a pointer: the pass
                // and the doors that reach this step nest it no deeper.
                Box::pin(self.start_fresh_life_locked(
                    identity,
                    &mut self.foreground_cancel.subscribe(),
                    super::FreshLife::Reset,
                    last,
                ))
                .await?;
            }
        }
        self.settle_first_boot_row_in_memory(identity);
        Ok(true)
    }

    /// The registration follows `name`'s row after a change of it committed
    /// in Core (a create, an update, an unarchive, an adoption into the
    /// store, an archive, a removal): a row this runtime has not registered
    /// is registered now, with its customizer tool dispatcher, so the
    /// member's next build carries the host customizer's tools; a row that
    /// changed is registered again; a name the host's roster registered
    /// that a row holds now (an adoption) is registered from its row, which
    /// is its one owner from then on; and the registration of a row Core no
    /// longer holds is forgotten. `Ok(true)` when the runtime serves the
    /// name afterwards, `Ok(false)` when it does not (the change removed the
    /// identity, or Core holds the name for no identity). It reads one row,
    /// sends Core no command and wakes nothing. The change path calls it
    /// once the change committed.
    ///
    /// The caller must NOT hold the identity's lifecycle lock: a row that
    /// changed or left is registered or forgotten under that lock, which is
    /// not reentrant.
    // Called by the identity change path, which is not in this crate yet:
    // until it is, only the rows call it.
    #[allow(dead_code)]
    pub(crate) async fn register_from_row(
        &self,
        name: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        // An archive or a removal ended the identity's member, a step Core
        // journaled: the projection of its side effects reads it now, not
        // at its next sweep. Any other change journals none, and the wake
        // costs one empty page read.
        self.kick_lifecycle_projection();
        let registered_by_roster = self
            .entries
            .read()
            .await
            .get(name)
            .is_some_and(|entry| !entry.created);
        let followed = if registered_by_roster && self.row_holds_name(name).await? {
            self.refresh_created_registration(name).await
        } else {
            self.admit_at_delivery_door(name).await
        };
        match followed {
            // Core holds the name for no identity this runtime serves (the
            // change removed it): the registration is gone, as asked.
            Err(IdentityRuntimeError::UnknownIdentity(_)) => Ok(false),
            followed => followed,
        }
    }

    /// Whether a door to `name` is the door of an identity this runtime
    /// serves: `Ok(true)` when `name` is registered, or when Core holds it as
    /// an identity whose record is its row, which is registered now from
    /// that row; `Ok(false)` when Core holds the name for no identity (it is
    /// a member of the mob's raw plane, if anything), and without a session
    /// bridge; and the door's typed refusal of a name Core holds for an
    /// identity this runtime does not serve
    /// ([`IdentityRuntime::unregistered_entry_refusal`]). A read Core could
    /// not serve refuses, retryable, never read as "no identity's". It takes
    /// no lifecycle lock, so a caller holding one may ask.
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
    /// the registration of an identity whose record is its row is read again
    /// from that row (one store read, no actor command). A row Core changed
    /// since (an update of its addressability, labels, profile or context)
    /// is registered now, and the registration of one Core no longer holds
    /// (removed since) is forgotten, the name then answered as one this
    /// runtime has not registered. A read Core could not serve refuses,
    /// retryable. The caller holds no lifecycle lock: a changed row is
    /// registered under the identity's.
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

    /// Read the row of `name`, registered from its row, again: see
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
                // Registered from this very row: nothing to change.
                if matches!(current, Some((true, true))) {
                    return Ok(true);
                }
                // The row is the identity's one owner: its spec replaces
                // what was registered, a roster's registration included.
                let lifecycle_lock = self.lifecycle_lock_for(name).await;
                let _lifecycle_guard = lifecycle_lock.lock().await;
                self.register_created_locked(spec).await;
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
                    // An entry whose row carries no spec this build reads is
                    // no identity this runtime serves.
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
                detail: format!("reading the identity's row in the mob's identity store: {error}"),
            }),
        }
    }

    /// Register `name`, whose record is its row, from that row; a
    /// registration a roster pass or another door made meanwhile stands. A
    /// name whose row Core no longer holds (removed meanwhile) or whose row
    /// carries no spec, and a name over a bridge that cannot read rows, is
    /// unknown to this runtime; a read that failed refuses, retryable. It
    /// takes no lifecycle lock: the registration is one step under the
    /// registration table's write lock.
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
                        "reading the identity's row in the mob's identity store: {error}"
                    ),
                });
            }
        };
        self.register_created_if_absent(spec).await;
        Ok(true)
    }

    /// Register the member Core is about to build, `identity`'s, when its
    /// record is its row: from that row, as a door would
    /// ([`Self::admit_unregistered_name`]), so Meta's build preparer
    /// prepares it as a registered identity's (the host's customizer runs,
    /// its tools are published into the identity's stable dispatcher, which
    /// Meta's spawn customizer attached to the build, and the build is
    /// recorded as the identity's). `Ok(false)` when Core holds no such
    /// entry of it, its row carries no spec, or the bridge cannot read rows.
    /// It runs inside Core's build of the member, so it never waits on the
    /// identity's lifecycle lock (a lifecycle operation holding it may be
    /// the one Core builds for): the registration is one step under the
    /// registration table's write lock, and one already made stands.
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
                "the identity's row in the mob's identity store could not be read: {error}"
            )),
        }
    }

    /// Record `spec`, a row's spec, as its identity's registration, while
    /// the caller holds its lifecycle lock. A wake refusal recorded for it
    /// survives, as for any re-registration.
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

    /// Record `spec`, a row's spec, as its identity's registration, unless
    /// the identity is registered already: the check and the record are one
    /// step under the registration table's write lock, so a registration
    /// made meanwhile (a roster pass's, a door's) stands. No lifecycle lock
    /// is taken or needed: nothing is replaced.
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

    /// The registration of `spec`, read from its identity's row.
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

    /// What a registration of `identity` keeps beside its spec: its event
    /// channel, kept across re-registrations (its subscribers belong to the
    /// identity's life, not to its record), and, when the host composes an
    /// agent customizer, its stable customizer tool dispatcher. Meta's spawn
    /// customizer attaches that dispatcher to every build of the identity's
    /// member, and the build preparer publishes the host customizer's tools
    /// into it.
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

    /// Forget the registration of `identity`, read from a row Core no
    /// longer held when the roster pass or a delivery door read it, under
    /// its lifecycle lock: only while its registration is still the row's
    /// and Core's entry of its name, read again under the lock, is no such
    /// entry (one a door registered after the pass's read stays). A read
    /// that fails keeps the registration for a later pass.
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
                    "Core's entry of the identity's name did not read; its registration stays \
                     until a later roster pass"
                );
                return;
            }
        }
        self.event_channels.write().await.remove(identity);
        self.entries.write().await.remove(identity);
    }

    /// The refusal of a delivery, a wake or a grant to `identity` when Core
    /// holds no member of it, answered from Core's entry of its name:
    /// archived, a final refusal naming the archive
    /// ([`IdentityRuntimeError::IdentityArchived`]: its unarchive seats it
    /// again); awaiting its host's preparation, its held creation or its
    /// seat, retryable ([`IdentityRuntimeError::IdentityWakeRetryable`]).
    /// `None` for any other entry (an identity its host creates on its first
    /// use included), and for one Core holds a member of, which the caller
    /// answers with its view state; a read Core could not serve refuses,
    /// retryable.
    pub(super) async fn unseated_refusal(
        &self,
        identity: &AgentIdentity,
    ) -> Option<IdentityRuntimeError> {
        let entry = match self.core_entry_of_name(identity).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return None,
            Err(error) => return Some(error),
        };
        if entry.member.is_some() {
            return None;
        }
        let detail = match entry.unseated? {
            meerkat_mob::UnseatedEntryCause::Archived => {
                return Some(IdentityRuntimeError::IdentityArchived {
                    identity: identity.clone(),
                });
            }
            meerkat_mob::UnseatedEntryCause::PreparationPending => {
                "the identity awaits its host's preparation: Core seats its member once the \
                 host settles it. Nothing was delivered; deliver again then"
            }
            meerkat_mob::UnseatedEntryCause::HeldCreation => {
                "the identity awaits its held creation by its host. Nothing was delivered; \
                 deliver again once its host created it"
            }
            meerkat_mob::UnseatedEntryCause::SeatOwed => {
                "Core owes the identity its seat. Nothing was delivered; deliver again once \
                 Core seated it"
            }
            _ => return None,
        };
        Some(IdentityRuntimeError::IdentityWakeRetryable {
            identity: identity.clone(),
            session_id: None,
            detail: detail.to_string(),
        })
    }
}
